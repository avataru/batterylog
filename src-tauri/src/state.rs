use std::sync::Mutex;

use batteries_core::db::Db;
use tera::Tera;

pub struct AppState {
    pub db: Mutex<Db>,
    pub tera: Tera,
}
