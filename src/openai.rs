use anyhow::Result;
use serde::{Deserialize, Serialize};

use crate::weather::WeatherInfo;

// Groq API совместим с OpenAI API — те же структуры запроса/ответа
#[derive(Debug, Serialize)]
struct ChatRequest {
    model: String,
    messages: Vec<Message>,
    max_tokens: u32,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
struct Message {
    role: String,
    content: String,
}

#[derive(Debug, Deserialize)]
struct ChatResponse {
    choices: Vec<Choice>,
}

#[derive(Debug, Deserialize)]
struct Choice {
    message: Message,
}

async fn groq_request(messages: Vec<Message>, max_tokens: u32) -> Result<String> {
    let api_key = std::env::var("GROQ_API_KEY")
        .map_err(|_| anyhow::anyhow!("GROQ_API_KEY не задан в .env"))?;

    let client = reqwest::Client::new();
    let body = ChatRequest {
        model: "llama-3.1-8b-instant".to_string(),
        messages,
        max_tokens,
    };

    let resp = client
        .post("https://api.groq.com/openai/v1/chat/completions")
        .bearer_auth(&api_key)
        .json(&body)
        .send()
        .await?;

    if !resp.status().is_success() {
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        return Err(anyhow::anyhow!("Groq API error {status}: {text}"));
    }

    let chat_resp = resp.json::<ChatResponse>().await?;
    Ok(chat_resp
        .choices
        .into_iter()
        .next()
        .map(|c| c.message.content)
        .unwrap_or_default())
}

/// Генерирует рекомендацию одежды
pub async fn get_outfit_advice(weather: &WeatherInfo) -> Result<String> {
    let prompt = format!(
        "Дай короткую (2-3 предложения) практичную рекомендацию что надеть сегодня. \
         Данные о погоде: температура {temp:.0}°C, ощущается как {feels:.0}°C, \
         {desc}, ветер {wind:.1} м/с, влажность {hum}%. \
         Отвечай по-русски, без лишних вступлений, только суть.",
        temp = weather.temp,
        feels = weather.feels_like,
        desc = weather.description,
        wind = weather.wind_speed,
        hum = weather.humidity,
    );

    match groq_request(vec![Message { role: "user".to_string(), content: prompt }], 150).await {
        Ok(text) => Ok(text),
        Err(e) => {
            log::warn!("Groq outfit error: {e}");
            Ok(weather.basic_outfit_advice())
        }
    }
}

/// Отвечает на произвольное сообщение пользователя (свободный чат)
pub async fn chat_with_ai(user_message: &str) -> Result<String> {
    groq_request(
        vec![
            Message {
                role: "system".to_string(),
                content: "Ты умный и дружелюбный помощник. Отвечай кратко и по делу, по-русски. \
                          Ты встроен в Telegram-бота который помогает школьнику с расписанием и планами на день."
                    .to_string(),
            },
            Message {
                role: "user".to_string(),
                content: user_message.to_string(),
            },
        ],
        600,
    )
    .await
}
