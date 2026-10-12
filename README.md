# 🚀 EDIBLS Telegram Bot

<div align="center">

![Rust](https://img.shields.io/badge/Rust-2024-DEA584?style=for-the-badge&logo=rust&logoColor=black)
![Docker](https://img.shields.io/badge/Docker-Compose-2496ED?style=for-the-badge&logo=docker&logoColor=white)
![Telegram](https://img.shields.io/badge/Telegram-Bot%20API-26A5E4?style=for-the-badge&logo=telegram&logoColor=white)
![yt-dlp](https://img.shields.io/badge/yt--dlp-Latest-FF0000?style=for-the-badge&logo=youtube&logoColor=white)

**Многофункциональный, высокопроизводительный Telegram-бот для скачивания медиаконтента и музыки в максимальном качестве.**

[Возможности](#-возможности) • [Команды](#-команды) • [Архитектура](#-архитектура) • [Установка и запуск](#-установка-и-запуск) • [Переменные окружения](#-переменные-окружения)

</div>

---

## ✨ Возможности

- 🎬 **Скачивание видео до 2 ГБ:** Загрузка роликов без водяных знаков в максимальном доступном разрешении (1080p, 720p, 480p и др.).
- 🎵 **Извлечение и скачивание аудио:** Автоматическая конвертация в MP3 с сохранением метаданных (ID3-теги, обложка, исполнитель, трек).
- ⚡ **Мгновенный кэш (0.1с):** Ранее скачанные медиафайлы отправляются повторно мгновенно через сохранённые `file_id` Telegram.
- 📊 **Плавный прогресс-бар:** Отображение реального прогресса скачивания (`📥 Скачивание: XX%`) и отправки (`📤 Отправка: XX%`).
- ⏱️ **Управление очередью:** Ограничение параллельных загрузок и изоляция сессий пользователей для стабильной работы сервера.
- 🛡️ **Обход блокировок YouTube:** Интеграция с локальным сервисом поставки PO-токенов (`bgutil-ytdlp-pot-provider`).

---

## 🌐 Поддерживаемые платформы

| Платформа | Видео | Аудио | Поиск через бота |
| :--- | :---: | :---: | :---: |
| **YouTube** (`youtube.com`, `youtu.be`, Shorts) | ✅ | ✅ | ✅ `/yt <запрос>` |
| **SoundCloud** (`soundcloud.com`, `on.soundcloud.com`) | ❌ | ✅ | ✅ `/sc <запрос>` |
| **Яндекс Музыка** (`music.yandex.ru`) | ❌ | ✅ | ✅ `/ym <запрос>` или текст в ЛС |
| **TikTok** (`tiktok.com`) | ✅ | ✅ | — |
| **Instagram** (`instagram.com` Reels, Stories, Posts) | ✅ | ✅ | — |
| **Pornhub** (`pornhub.com`, `thumbzilla.com`) | ✅ | ✅ | ✅ `/ph <запрос>` |
| **VK Видео / Клипы** (`vk.com`, `vkvideo.ru`) | ✅ | ✅ | — |
| **Twitter / X** (`x.com`, `twitter.com`) | ✅ | ✅ | — |
| **Reddit**, **Pinterest**, **Twitch Clps** и ещё 1000+ сайтов | ✅ | ✅ | — |

---

## ⌨️ Команды бота

| Команда | Описание |
| :--- | :--- |
| `/start`, `/help` | Справка по возможностям и поддерживаемым сервисам |
| `/settings` | Настройки качества (Авто / Выбор вручную) и режима «Только аудио» |
| `/audio <ссылка>` | Принудительное скачивание только аудиодорожки (MP3) по ссылке |
| `/yt <запрос>` | Поиск видео на YouTube с инлайн-клавиатурой результатов |
| `/sc <запрос>` | Поиск треков на SoundCloud с выводом длительности и исполнителя |
| `/ym <запрос>` | Поиск треков на Яндекс Музыке (в ЛС доступен поиск простым текстом) |
| `/ph <запрос>` | Поиск роликов на Pornhub с инлайн-выбором |

---

## 🏗 Архитектура

Проект оптимизирован под работу в Linux-окружении с минимальным потреблением ресурсов:

```
                            ┌────────────────────────┐
                            │      Telegram Bot      │
                            └───────────┬────────────┘
                                        │ (MTProto / Bot API)
                                        ▼
                            ┌────────────────────────┐
                            │    telegram-bot-api    │ (Локальный сервер Telegram API,
                            │       (:8081)          │  снятие лимита 50МБ -> 2000МБ)
                            └───────────┬────────────┘
                                        │
┌───────────────────────────────────────┴────────────────────────────────────────┐
│  edibls (Rust / Teloxide daemon)                                              │
│                                                                                │
│  ├── Media Detection & Router (URL / Commands / Private chat queries)         │
│  ├── Queue Manager (Domain locks + Waiter slots)                              │
│  ├── File ID Cache (Zero-latency instant re-delivery)                         │
│  ├── Providers:                                                               │
│  │   ├── yt-dlp (YouTube, SoundCloud, TikTok, IG, etc.)                       │
│  │   ├── bgutil-pot-provider (:4416 - YouTube PO Token integration)           │
│  │   ├── yandex-music-downloader (Native FLAC/MP3 pipeline with progress)    │
│  │   └── pornhub parser                                                       │
│  └── Tagging & Metadata (lofty, image thumbnail embedding)                     │
└────────────────────────────────────────────────────────────────────────────────┘
```

---

## 🚀 Установка и запуск

### 1. Клонирование репозитория
```bash
git clone git@github.com:f02xygen/edibls.git
cd edibls
```

### 2. Настройка конфигурации (`.env`)
Создайте файл `.env` на основе примера:
```env
# Токен бота от @BotFather (обязательно)
TELOXIDE_TOKEN=1234567890:ABCdefGHIjklMNOpqrSTUvwxYZ

# Учетные данные Telegram API для локального telegram-bot-api (my.telegram.org)
TELEGRAM_API_ID=2040
TELEGRAM_API_HASH=b18441a1ff607e10a989891a5462e627

# Токен Яндекс Музыки (для поиска и скачивания в высоком качестве)
YANDEX_MUSIC_TOKEN=y0_AgAAAA...

# Лимиты загрузок (опционально)
MAX_FILESIZE_MB=2000
MAX_DURATION_SECS=7200
MAX_CONCURRENT_DOWNLOADS=3
```

### 3. Запуск через Docker Compose
```bash
docker compose up -d
```

Проверить статус и логи:
```bash
docker compose ps
docker compose logs -f edibls
```

---

## 🛠 Локальная сборка для слабых серверов

Если сервер имеет ограниченный объем RAM (<2 ГБ) и сборка Rust-проекта завершается OOM-киллером, образ собирается локально:

```bash
# 1. Локальная сборка
docker build -t edibls:latest .

# 2. Экспорт и сжатие
docker save edibls:latest | zstd -3 -T0 > edibls_latest.tar.zst

# 3. Передача на сервер
rsync -avP edibls_latest.tar.zst user@server:~/

# 4. Загрузка на сервере
ssh user@server "zstd -dc ~/edibls_latest.tar.zst | docker load && rm ~/edibls_latest.tar.zst"
```

---

## 📄 Лицензия

MIT License © 2026 [f02xygen](https://github.com/f02xygen)
