use std::sync::Mutex;

pub(crate) static ENV_LOCK: Mutex<()> = Mutex::new(());
