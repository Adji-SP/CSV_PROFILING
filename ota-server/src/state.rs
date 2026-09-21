use std::sync::Arc;

use crate::{config::Config, services::BuildCoordinator, storage::Storage};

#[derive(Clone)]
pub struct AppState {
    pub console: Arc<crate::console::Console>,
    pub config: Arc<Config>,
    pub storage: Storage,
    pub coordinator: Arc<BuildCoordinator>,
}
