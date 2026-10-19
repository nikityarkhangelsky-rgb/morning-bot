use anyhow::Result;
use serde::Deserialize;

#[derive(Debug, Deserialize)]
struct OWMResponse {
    weather: Vec<OWMWeather>,
    main: OWMMain,
    wind: OWMWind,
    name: String,
}

#[derive(Debug, Deserialize)]
struct OWMWeather {
    description: String,
}

#[derive(Debug, Deserialize)]
struct OWMMain {
    temp: f64,
    feels_like: f64,
    humidity: u32,
}

#[derive(Debug, Deserialize)]
struct OWMWind {
    speed: f64,
}

#[derive(Debug, Clone)]
pub struct WeatherInfo {
    pub city: String,
    pub temp: f64,
    pub feels_like: f64,
    pub description: String,
    pub humidity: u32,
    pub wind_speed: f64,
}

impl WeatherInfo {
    /// Возвращает эмодзи для описания погоды
    pub fn emoji(&self) -> &'static str {
        let desc = self.description.to_lowercase();
        if desc.contains("гроз") || desc.contains("storm") || desc.contains("thunder") {
            "⛈"
        } else if desc.contains("дожд") || desc.contains("rain") || desc.contains("drizzle") {
            "🌧"
        } else if desc.contains("снег") || desc.contains("snow") {
            "❄️"
        } else if desc.contains("туман") || desc.contains("fog") || desc.contains("mist") {
            "🌫"
        } else if desc.contains("облач") || desc.contains("cloud") || desc.contains("overcast") {
            "☁️"
        } else if desc.contains("малооблач") || desc.contains("few clouds") {
            "🌤"
        } else {
            "☀️"
        }
    }

    /// Простая рекомендация одежды по температуре (запасной вариант без AI)
    pub fn basic_outfit_advice(&self) -> String {
        let t = self.temp;
        let windy = self.wind_speed > 7.0;
        let wet = self.description.to_lowercase().contains("rain")
            || self.description.to_lowercase().contains("дожд");

        let mut advice = if t < 0.0 {
            "🧥 Очень холодно — нужна зимняя куртка, шапка и перчатки.".to_string()
        } else if t < 5.0 {
            "🧥 Холодно — тёплая куртка, шапка.".to_string()
        } else if t < 10.0 {
            "🧣 Прохладно — куртка и шарф.".to_string()
        } else if t < 15.0 {
            "🧥 Нежарко — лёгкая куртка или толстовка.".to_string()
        } else if t < 20.0 {
            "👕 Умеренно тепло — свитер или толстовка.".to_string()
        } else if t < 25.0 {
            "👕 Тепло — футболка, лёгкая одежда.".to_string()
        } else {
            "😎 Жарко — лёгкая одежда, не забудь воду!".to_string()
        };

        if windy {
            advice.push_str(" Ветрено — возьми ветровку.");
        }
        if wet {
            advice.push_str(" Дождь — зонт обязателен! ☂️");
        }
        advice
    }

    pub fn to_summary_text(&self) -> String {
        format!(
            "{emoji} <b>Погода в {city}</b>\n\
             🌡 Температура: <b>{temp:.0}°C</b> (ощущается как {feels:.0}°C)\n\
             💧 Влажность: {hum}%\n\
             💨 Ветер: {wind:.1} м/с\n\
             📝 {desc}",
            emoji = self.emoji(),
            city = self.city,
            temp = self.temp,
            feels = self.feels_like,
            hum = self.humidity,
            wind = self.wind_speed,
            desc = capitalize(&self.description),
        )
    }
}

fn capitalize(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        None => String::new(),
        Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
    }
}

pub async fn fetch_weather(lat: f64, lon: f64) -> Result<WeatherInfo> {
    let api_key = std::env::var("OPENWEATHER_API_KEY")
        .expect("OPENWEATHER_API_KEY не задан в .env");

    let url = format!(
        "https://api.openweathermap.org/data/2.5/weather\
         ?lat={lat}&lon={lon}&appid={key}&units=metric&lang=ru",
        lat = lat,
        lon = lon,
        key = api_key,
    );

    let client = reqwest::Client::new();
    let resp = client.get(&url).send().await?.json::<OWMResponse>().await?;

    Ok(WeatherInfo {
        city: resp.name,
        temp: resp.main.temp,
        feels_like: resp.main.feels_like,
        description: resp.weather.first().map(|w| w.description.clone()).unwrap_or_default(),
        humidity: resp.main.humidity,
        wind_speed: resp.wind.speed,
    })
}
