mod db;
mod weather;
mod openai;
mod scheduler;
mod handlers;

use teloxide::prelude::*;
use anyhow::Result;
use std::sync::Arc;
use tokio::sync::Mutex;

fn main() -> Result<()> {
    pretty_env_logger::init();
    dotenv::dotenv().ok();

    // Запускаем всё в отдельном потоке с увеличенным стеком (16MB)
    // чтобы не было STATUS_STACK_OVERFLOW в debug-сборке
    let builder = std::thread::Builder::new().stack_size(16 * 1024 * 1024);
    let handler = builder.spawn(|| {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .thread_stack_size(8 * 1024 * 1024)
            .enable_all()
            .build()
            .expect("Не удалось создать tokio runtime");
        rt.block_on(run()).expect("Ошибка в run()");
    })?;

    handler.join().expect("Поток упал");
    Ok(())
}

async fn run() -> Result<()> {
    log::info!("Запуск утреннего бота...");

    let bot = Bot::from_env();
    let database = Arc::new(Mutex::new(db::Database::new("bot_data.db")?));

    // Запускаем планировщик утренних сообщений
    let db_clone = Arc::clone(&database);
    let bot_clone = bot.clone();
    tokio::spawn(async move {
        if let Err(e) = scheduler::start_morning_scheduler(bot_clone, db_clone).await {
            log::error!("Ошибка планировщика: {}", e);
        }
    });

    // Запускаем бота
    let db_for_handler = Arc::clone(&database);
    let handler = handlers::build_handler();

    Dispatcher::builder(bot, handler)
        .dependencies(dptree::deps![db_for_handler])
        .enable_ctrlc_handler()
        .build()
        .dispatch()
        .await;

    Ok(())
}
