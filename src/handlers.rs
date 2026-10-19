use std::sync::Arc;
use tokio::sync::Mutex;
use teloxide::prelude::*;
use teloxide::types::{
    KeyboardButton, KeyboardMarkup,
    ParseMode, ButtonRequest,
};
use teloxide::utils::command::BotCommands;
use chrono::Local;

use crate::db::{Database, UserData, ScheduleEntry, Task};
use crate::scheduler::send_morning_summary;
use crate::openai::chat_with_ai;

type DbState = Arc<Mutex<Database>>;
type HandlerResult = Result<(), Box<dyn std::error::Error + Send + Sync>>;

// ──────────────────── Команды ────────────────────

#[derive(BotCommands, Clone)]
#[command(rename_rule = "lowercase", description = "Команды бота:")]
pub enum Command {
    #[command(description = "Показать это сообщение")]
    Help,
    #[command(description = "Начать работу / обновить локацию")]
    Start,
    #[command(description = "Получить утреннюю сводку прямо сейчас")]
    Now,
    #[command(description = "Добавить урок: /урок Пн 08:30 Математика")]
    Урок,
    #[command(description = "Показать расписание на неделю")]
    Расписание,
    #[command(description = "Очистить всё расписание")]
    Очистить,
    #[command(description = "Добавить задачу на сегодня: /задача Купить хлеб")]
    Задача,
    #[command(description = "Добавить задачу на дату: /задачана 2024-12-25 Задача")]
    Задачана,
    #[command(description = "Показать задачи на сегодня")]
    Задачи,
    #[command(description = "Отметить задачу выполненной: /готово 3")]
    Готово,
    #[command(description = "Удалить задачу: /удалить 3")]
    Удалить,
}

// ──────────────────── Handler builder ────────────────────

pub fn build_handler() -> teloxide::dispatching::UpdateHandler<Box<dyn std::error::Error + Send + Sync + 'static>> {
    use teloxide::dispatching::UpdateFilterExt;
    use dptree::case;

    let command_handler = teloxide::filter_command::<Command, _>()
        .branch(case![Command::Help].endpoint(cmd_help))
        .branch(case![Command::Start].endpoint(cmd_start))
        .branch(case![Command::Now].endpoint(cmd_now))
        .branch(case![Command::Урок].endpoint(cmd_add_lesson))
        .branch(case![Command::Расписание].endpoint(cmd_show_schedule))
        .branch(case![Command::Очистить].endpoint(cmd_clear_schedule))
        .branch(case![Command::Задача].endpoint(cmd_add_task_today))
        .branch(case![Command::Задачана].endpoint(cmd_add_task_date))
        .branch(case![Command::Задачи].endpoint(cmd_show_tasks))
        .branch(case![Command::Готово].endpoint(cmd_done_task))
        .branch(case![Command::Удалить].endpoint(cmd_delete_task));

    Update::filter_message()
        .branch(command_handler)
        .branch(
            dptree::filter(|msg: Message| msg.location().is_some())
                .endpoint(handle_location),
        )
        .branch(
            dptree::filter(|msg: Message| msg.text().is_some())
                .endpoint(handle_ai_chat),
        )
}

// ──────────────────── Command handlers ────────────────────

async fn cmd_help(bot: Bot, msg: Message) -> HandlerResult {
    bot.send_message(msg.chat.id, Command::descriptions().to_string()).await?;
    Ok(())
}

async fn cmd_start(bot: Bot, msg: Message) -> HandlerResult {
    let keyboard = KeyboardMarkup::new(vec![vec![
        KeyboardButton {
            text: "📍 Отправить мою геолокацию".to_string(),
            request: Some(ButtonRequest::Location),
        },
    ]])
    .resize_keyboard();

    bot.send_message(
        msg.chat.id,
        "👋 Привет\\! Я буду присылать тебе утреннюю сводку каждый день в *07:30*\\.\n\n\
         Сначала отправь мне свою геолокацию, чтобы я мог узнать погоду в твоём городе:",
    )
    .parse_mode(ParseMode::MarkdownV2)
    .reply_markup(keyboard)
    .await?;
    Ok(())
}

async fn handle_location(
    bot: Bot,
    msg: Message,
    db: DbState,
) -> HandlerResult {
    let location = msg.location().unwrap();
    let user_data = UserData {
        chat_id: msg.chat.id.0,
        lat: location.latitude,
        lon: location.longitude,
        city_name: None,
    };
    {
        let db = db.lock().await;
        if let Err(e) = db.upsert_user(&user_data) {
            log::error!("DB error: {e}");
        }
    }

    bot.send_message(
        msg.chat.id,
        "✅ Геолокация сохранена\\! Теперь я буду присылать тебе сводку каждое утро в *07:30*\\.\n\
         Напиши /now чтобы получить сводку прямо сейчас\\.",
    )
    .parse_mode(ParseMode::MarkdownV2)
    .reply_markup(teloxide::types::KeyboardRemove::new())
    .await?;
    Ok(())
}

async fn cmd_now(bot: Bot, msg: Message, db: DbState) -> HandlerResult {
    let user = {
        let db = db.lock().await;
        db.get_user(msg.chat.id.0).ok().flatten()
    };

    match user {
        Some(u) => {
            bot.send_message(msg.chat.id, "⏳ Собираю данные...").await?;
            if let Err(e) =
                send_morning_summary(&bot, msg.chat.id, u.lat, u.lon, &db).await
            {
                log::error!("{e}");
                bot.send_message(msg.chat.id, "❌ Ошибка при получении данных.").await?;
            }
        }
        None => {
            bot.send_message(
                msg.chat.id,
                "⚠️ Сначала отправь свою геолокацию с помощью /start",
            )
            .await?;
        }
    }
    Ok(())
}

// ──────────────────── Schedule commands ────────────────────

/// Парсит аббревиатуру дня недели → индекс (0=Пн)
fn parse_day(s: &str) -> Option<u8> {
    match s.to_lowercase().as_str() {
        "пн" | "mon" => Some(0),
        "вт" | "tue" => Some(1),
        "ср" | "wed" => Some(2),
        "чт" | "thu" => Some(3),
        "пт" | "fri" => Some(4),
        "сб" | "sat" => Some(5),
        "вс" | "sun" => Some(6),
        _ => None,
    }
}

const DAY_ABBREV: [&str; 7] = ["Пн", "Вт", "Ср", "Чт", "Пт", "Сб", "Вс"];

async fn cmd_add_lesson(
    bot: Bot,
    msg: Message,
    db: DbState,
) -> HandlerResult {
    // Формат: /урок Пн 08:30 Математика
    let text = msg.text().unwrap_or("");
    let parts: Vec<&str> = text.splitn(4, ' ').collect();

    if parts.len() < 4 {
        bot.send_message(
            msg.chat.id,
            "❌ Формат: `/урок Пн 08:30 Математика`",
        )
        .parse_mode(ParseMode::MarkdownV2)
        .await?;
        return Ok(());
    }

    let day_str = parts[1];
    let time_str = parts[2];
    let subject = parts[3];

    let Some(day) = parse_day(day_str) else {
        bot.send_message(
            msg.chat.id,
            "❌ День недели: Пн, Вт, Ср, Чт, Пт, Сб, Вс",
        )
        .await?;
        return Ok(());
    };

    let entry = ScheduleEntry {
        id: None,
        chat_id: msg.chat.id.0,
        day_of_week: day,
        time_str: time_str.to_string(),
        subject: subject.to_string(),
    };

    match db.lock().await.add_schedule_entry(&entry) {
        Ok(id) => {
            bot.send_message(
                msg.chat.id,
                format!(
                    "✅ Добавлено \\[\\#{}\\]: {} в {} \\({}\\)",
                    id,
                    escape_md(subject),
                    escape_md(time_str),
                    DAY_ABBREV[day as usize]
                ),
            )
            .parse_mode(ParseMode::MarkdownV2)
            .await?;
        }
        Err(e) => {
            log::error!("{e}");
            bot.send_message(msg.chat.id, "❌ Ошибка сохранения").await?;
        }
    }
    Ok(())
}

async fn cmd_show_schedule(
    bot: Bot,
    msg: Message,
    db: DbState,
) -> HandlerResult {
    let db = db.lock().await;
    let mut lines = vec!["📅 *Расписание на неделю:*\n".to_string()];
    let mut has_any = false;

    for day in 0u8..7 {
        let entries = db
            .get_schedule_for_day(msg.chat.id.0, day)
            .unwrap_or_default();
        if !entries.is_empty() {
            has_any = true;
            lines.push(format!("*{}:*", DAY_ABBREV[day as usize]));
            for e in &entries {
                lines.push(format!(
                    "  `#{}` {} — {}",
                    e.id.unwrap_or(0),
                    e.time_str,
                    escape_md(&e.subject)
                ));
            }
        }
    }

    if !has_any {
        lines.push("Расписание пустое\\. Добавь уроки через /урок".to_string());
    }

    bot.send_message(msg.chat.id, lines.join("\n"))
        .parse_mode(ParseMode::MarkdownV2)
        .await?;
    Ok(())
}

async fn cmd_clear_schedule(
    bot: Bot,
    msg: Message,
    db: DbState,
) -> HandlerResult {
    db.lock().await.clear_schedule(msg.chat.id.0).ok();
    bot.send_message(msg.chat.id, "🗑 Расписание очищено.").await?;
    Ok(())
}

// ──────────────────── Task commands ────────────────────

async fn cmd_add_task_today(
    bot: Bot,
    msg: Message,
    db: DbState,
) -> HandlerResult {
    let text = msg.text().unwrap_or("");
    let task_text = text.splitn(2, ' ').nth(1).unwrap_or("").trim();

    if task_text.is_empty() {
        bot.send_message(msg.chat.id, "❌ Формат: `/задача Текст задачи`")
            .parse_mode(ParseMode::MarkdownV2)
            .await?;
        return Ok(());
    }

    let today = Local::now().format("%Y-%m-%d").to_string();
    add_task_for_date(&bot, &msg, &db, &today, task_text).await
}

async fn cmd_add_task_date(
    bot: Bot,
    msg: Message,
    db: DbState,
) -> HandlerResult {
    // Формат: /задачана 2024-12-25 Текст задачи
    let text = msg.text().unwrap_or("");
    let parts: Vec<&str> = text.splitn(3, ' ').collect();

    if parts.len() < 3 {
        bot.send_message(
            msg.chat.id,
            "❌ Формат: `/задачана 2024\\-12\\-25 Текст задачи`",
        )
        .parse_mode(ParseMode::MarkdownV2)
        .await?;
        return Ok(());
    }

    let date = parts[1];
    let task_text = parts[2].trim();
    add_task_for_date(&bot, &msg, &db, date, task_text).await
}

async fn add_task_for_date(
    bot: &Bot,
    msg: &Message,
    db: &DbState,
    date: &str,
    task_text: &str,
) -> HandlerResult {
    let task = Task {
        id: None,
        chat_id: msg.chat.id.0,
        date: date.to_string(),
        text: task_text.to_string(),
        done: false,
    };

    match db.lock().await.add_task(&task) {
        Ok(id) => {
            bot.send_message(
                msg.chat.id,
                format!(
                    "✅ Задача \\#{}  добавлена на {}: {}",
                    id,
                    escape_md(date),
                    escape_md(task_text)
                ),
            )
            .parse_mode(ParseMode::MarkdownV2)
            .await?;
        }
        Err(e) => {
            log::error!("{e}");
            bot.send_message(msg.chat.id, "❌ Ошибка сохранения задачи.").await?;
        }
    }
    Ok(())
}

async fn cmd_show_tasks(
    bot: Bot,
    msg: Message,
    db: DbState,
) -> HandlerResult {
    let today = Local::now().format("%Y-%m-%d").to_string();
    let tasks = {
        let db = db.lock().await;
        db.get_tasks_for_date(msg.chat.id.0, &today)
            .unwrap_or_default()
    };

    if tasks.is_empty() {
        bot.send_message(msg.chat.id, "📭 На сегодня задач нет.").await?;
    } else {
        let lines: Vec<String> = tasks
            .iter()
            .map(|t| {
                let check = if t.done { "✅" } else { "⬜" };
                let id = t.id.unwrap_or(0);
                format!("{} \\#{}  {}", check, id, escape_md(&t.text))
            })
            .collect();

        bot.send_message(
            msg.chat.id,
            format!("📝 *Задачи на сегодня:*\n{}", lines.join("\n")),
        )
        .parse_mode(ParseMode::MarkdownV2)
        .await?;
    }
    Ok(())
}

async fn cmd_done_task(
    bot: Bot,
    msg: Message,
    db: DbState,
) -> HandlerResult {
    parse_task_id_and_run(&bot, &msg, &db, |db, id| {
        db.complete_task(msg.chat.id.0, id)
    }, "✅ Задача отмечена выполненной!")
    .await
}

async fn cmd_delete_task(
    bot: Bot,
    msg: Message,
    db: DbState,
) -> HandlerResult {
    parse_task_id_and_run(&bot, &msg, &db, |db, id| {
        db.delete_task(msg.chat.id.0, id)
    }, "🗑 Задача удалена.")
    .await
}

async fn parse_task_id_and_run<F>(
    bot: &Bot,
    msg: &Message,
    db: &DbState,
    action: F,
    success_msg: &str,
) -> HandlerResult
where
    F: FnOnce(&Database, i64) -> anyhow::Result<()>,
{
    let text = msg.text().unwrap_or("");
    let id_str = text.split_whitespace().nth(1).unwrap_or("");

    match id_str.parse::<i64>() {
        Ok(id) => {
            let db_guard = db.lock().await;
            match action(&db_guard, id) {
                Ok(_) => {
                    bot.send_message(msg.chat.id, success_msg).await?;
                }
                Err(e) => {
                    log::error!("{e}");
                    bot.send_message(msg.chat.id, "❌ Ошибка.").await?;
                }
            }
        }
        Err(_) => {
            bot.send_message(msg.chat.id, "❌ Укажи номер задачи: `/готово 3`")
                .parse_mode(ParseMode::MarkdownV2)
                .await?;
        }
    }
    Ok(())
}

// ──────────────────── AI Chat ────────────────────

async fn handle_ai_chat(bot: Bot, msg: Message) -> HandlerResult {
    let text = match msg.text() {
        Some(t) => t,
        None => return Ok(()),
    };

    // Показываем индикатор печати
    bot.send_chat_action(msg.chat.id, teloxide::types::ChatAction::Typing)
        .await?;

    match chat_with_ai(text).await {
        Ok(reply) => {
            bot.send_message(msg.chat.id, reply).await?;
        }
        Err(e) => {
            log::error!("AI chat error: {e}");
            bot.send_message(
                msg.chat.id,
                "🤔 Не могу ответить прямо сейчас, попробуй позже.",
            )
            .await?;
        }
    }
    Ok(())
}

// ──────────────────── Utils ────────────────────

/// Экранирует спецсимволы для MarkdownV2
fn escape_md(s: &str) -> String {
    s.chars()
        .flat_map(|c| {
            if "_*[]()~`>#+-=|{}.!\\".contains(c) {
                vec!['\\', c]
            } else {
                vec![c]
            }
        })
        .collect()
}
