use anyhow::Result;
use chrono::{Local, Datelike, Timelike, NaiveTime};
use std::sync::Arc;
use tokio::sync::Mutex;
use tokio_cron_scheduler::{Job, JobScheduler};
use teloxide::prelude::*;
use teloxide::types::ParseMode;

use crate::db::Database;
use crate::weather;
use crate::openai;

const DAY_NAMES_FULL: [&str; 7] = [
    "Понедельник", "Вторник", "Среда", "Четверг", "Пятница", "Суббота", "Воскресенье",
];

/// Собирает и отправляет утреннюю сводку одному пользователю
pub async fn send_morning_summary(
    bot: &Bot,
    chat_id: ChatId,
    lat: f64,
    lon: f64,
    db: &Arc<Mutex<Database>>,
) -> Result<()> {
    let now = Local::now();
    let weekday_idx = now.weekday().num_days_from_monday() as u8;
    let today_str = now.format("%Y-%m-%d").to_string();
    let day_name = DAY_NAMES_FULL[weekday_idx as usize];

    // 1. Погода
    let weather = weather::fetch_weather(lat, lon).await;
    let (weather_text, outfit_text) = match weather {
        Ok(w) => {
            let outfit = match openai::get_outfit_advice(&w).await {
                Ok(advice) => advice,
                Err(e) => {
                    log::warn!("Groq error: {e}");
                    w.basic_outfit_advice()
                }
            };
            (w.to_summary_text(), outfit)
        }
        Err(e) => {
            log::error!("Weather fetch error: {e}");
            (
                "🌡 Не удалось получить погоду".to_string(),
                "👕 Одевайся по обстановке!".to_string(),
            )
        }
    };

    // 2. Расписание на сегодня
    let schedule_entries = {
        let db = db.lock().await;
        db.get_schedule_for_day(chat_id.0, weekday_idx).unwrap_or_default()
    };

    let schedule_text = if schedule_entries.is_empty() {
        "📭 Занятий нет".to_string()
    } else {
        schedule_entries
            .iter()
            .enumerate()
            .map(|(i, e)| format!("{}. {} — {}", i + 1, e.time_str, e.subject))
            .collect::<Vec<_>>()
            .join("\n")
    };

    // Находим время последнего урока
    let last_lesson_time = schedule_entries.last().map(|e| e.time_str.clone());

    // 3. Задачи на сегодня
    let tasks = {
        let db = db.lock().await;
        db.get_tasks_for_date(chat_id.0, &today_str).unwrap_or_default()
    };

    let tasks_text = if tasks.is_empty() {
        "✅ Задач нет".to_string()
    } else {
        tasks
            .iter()
            .map(|t| {
                let check = if t.done { "✅" } else { "⬜" };
                let id = t.id.unwrap_or(0);
                format!("{check} [#{id}] {}", t.text)
            })
            .collect::<Vec<_>>()
            .join("\n")
    };

    let hour = now.hour();
    let greeting = if hour < 12 { "Доброе утро" } else { "Добрый день" };

    // Подсчёт количества уроков и конец учебного дня
    let school_end = match &last_lesson_time {
        Some(t) => format!("🔔 Конец учебного дня: <b>{t}</b>"),
        None => String::new(),
    };

    let lesson_count = schedule_entries.len();
    let lesson_word = match lesson_count {
        1 => "урок",
        2..=4 => "урока",
        _ => "уроков",
    };

    let message = format!(
        "☀️ <b>{greeting}!</b> Сегодня <b>{day}</b>, {date}\n\
         \n\
         {weather}\n\
         \n\
         👕 <b>Что надеть:</b>\n\
         {outfit}\n\
         \n\
         📚 <b>Расписание</b> — {count} {word}:\n\
         {schedule}\n\
         {school_end}\n\
         \n\
         📝 <b>Задачи на сегодня:</b>\n\
         {tasks}",
        greeting = greeting,
        day = day_name,
        date = now.format("%d.%m.%Y"),
        weather = weather_text,
        outfit = outfit_text,
        count = lesson_count,
        word = lesson_word,
        schedule = schedule_text,
        school_end = school_end,
        tasks = tasks_text,
    );

    bot.send_message(chat_id, message)
        .parse_mode(ParseMode::Html)
        .await?;

    Ok(())
}

/// Проверяет расписание и отправляет напоминания о ближайших уроках
/// Вызывается каждую минуту
async fn check_and_send_reminders(
    bot: &Bot,
    db: &Arc<Mutex<Database>>,
) {
    let now = Local::now();
    let weekday_idx = now.weekday().num_days_from_monday() as u8;

    // Пропускаем выходные
    if weekday_idx >= 5 {
        return;
    }

    let current_time = NaiveTime::from_hms_opt(now.hour(), now.minute(), 0).unwrap();
    // Время через 10 минут
    let in_10 = NaiveTime::from_hms_opt(now.hour(), now.minute() + 10, 0)
        .or_else(|| {
            // Если минуты >= 60 — переходим на следующий час
            let h = now.hour() + 1;
            let m = (now.minute() + 10) % 60;
            NaiveTime::from_hms_opt(h, m, 0)
        });

    let Some(in_10) = in_10 else { return };

    let users = {
        let db = db.lock().await;
        db.get_all_users().unwrap_or_default()
    };

    for user in &users {
        let chat_id = ChatId(user.chat_id);
        let entries = {
            let db = db.lock().await;
            db.get_schedule_for_day(user.chat_id, weekday_idx).unwrap_or_default()
        };

        if entries.is_empty() {
            continue;
        }

        // Ищем урок который начнётся ровно через 10 минут
        for (idx, entry) in entries.iter().enumerate() {
            let Ok(lesson_time) = NaiveTime::parse_from_str(&entry.time_str, "%H:%M") else {
                continue;
            };

            if lesson_time == in_10 {
                // Следующий урок после этого
                let next = entries.get(idx + 1);
                let next_text = match next {
                    Some(n) => format!("\n➡️ После него: {} — {}", n.time_str, n.subject),
                    None => "\n🏁 Это последний урок сегодня!".to_string(),
                };

                let is_last = next.is_none();
                let emoji = if is_last { "🏃" } else { "⏰" };

                let msg = format!(
                    "{emoji} <b>Через 10 минут:</b> {subject}\n\
                     🕐 Начало в {time}{next}",
                    emoji = emoji,
                    subject = entry.subject,
                    time = entry.time_str,
                    next = next_text,
                );

                if let Err(e) = bot.send_message(chat_id, msg)
                    .parse_mode(ParseMode::Html)
                    .await
                {
                    log::error!("Ошибка отправки напоминания: {e}");
                }
            }
        }

        // Проверяем — закончился ли последний урок (через 1 минуту после его начала + 45 мин)
        // Считаем что урок длится 45 минут, отправляем когда заканчивается
        if let Some(last) = entries.last() {
            let Ok(last_time) = NaiveTime::parse_from_str(&last.time_str, "%H:%M") else {
                continue;
            };
            // Конец последнего урока = начало + 45 минут
            let end_min = last_time.minute() + 45;
            let end_time = NaiveTime::from_hms_opt(
                last_time.hour() + end_min / 60,
                end_min % 60,
                0,
            );

            if let Some(end) = end_time {
                if current_time == end {
                    let msg = format!(
                        "🏠 <b>Учебный день окончен!</b>\n\
                         Сегодня было {} {word}. Отдыхай! 🎉",
                        entries.len(),
                        word = match entries.len() {
                            1 => "урок",
                            2..=4 => "урока",
                            _ => "уроков",
                        }
                    );
                    if let Err(e) = bot.send_message(chat_id, msg)
                        .parse_mode(ParseMode::Html)
                        .await
                    {
                        log::error!("Ошибка отправки конца дня: {e}");
                    }
                }
            }
        }
    }
}

/// Запускает все планировщики
pub async fn start_morning_scheduler(
    bot: Bot,
    db: Arc<Mutex<Database>>,
) -> Result<()> {
    let scheduler = JobScheduler::new().await?;

    // 1. Утренняя сводка в 07:30
    {
        let bot = bot.clone();
        let db = Arc::clone(&db);
        let job = Job::new_async("0 30 7 * * Mon-Fri", move |_uuid, _lock| {
            let bot = bot.clone();
            let db = Arc::clone(&db);
            Box::pin(async move {
                log::info!("Запускаю утреннюю рассылку...");
                let users = {
                    let db = db.lock().await;
                    db.get_all_users().unwrap_or_default()
                };
                for user in users {
                    let chat_id = ChatId(user.chat_id);
                    if let Err(e) =
                        send_morning_summary(&bot, chat_id, user.lat, user.lon, &db).await
                    {
                        log::error!("Ошибка отправки для {}: {}", user.chat_id, e);
                    }
                }
            })
        })?;
        scheduler.add(job).await?;
    }

    // 2. Напоминания о уроках — каждую минуту в будние дни (8:00–17:00)
    {
        let bot = bot.clone();
        let db = Arc::clone(&db);
        let job = Job::new_async("0 * 8-17 * * Mon-Fri", move |_uuid, _lock| {
            let bot = bot.clone();
            let db = Arc::clone(&db);
            Box::pin(async move {
                check_and_send_reminders(&bot, &db).await;
            })
        })?;
        scheduler.add(job).await?;
    }

    scheduler.start().await?;

    // Держим планировщик живым
    loop {
        tokio::time::sleep(tokio::time::Duration::from_secs(60)).await;
    }
}
