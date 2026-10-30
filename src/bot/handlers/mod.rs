pub mod callbacks;
pub mod commands;
pub mod inline;
pub mod messages;
pub mod replies;
pub mod search;

#[allow(unused_imports)]
pub use callbacks::handle_callback_action;
#[allow(unused_imports)]
pub use commands::handle_command;
#[allow(unused_imports)]
pub use inline::{handle_inline_query, init_bot_username};
#[allow(unused_imports)]
pub use messages::handle_message;
#[allow(unused_imports)]
pub use replies::handle_reply;
pub use search::{
    handle_pornhub_search, handle_soundcloud_search, handle_yandex_search, handle_youtube_search,
};
