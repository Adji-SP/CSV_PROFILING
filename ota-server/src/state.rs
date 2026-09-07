use std::sync::Arc;

use crate::{config::Config, services::BuildCoordinator, storage::Storage};

#[derive(Clone)]
pub struct AppState {
    pub config: Arc<Config>,
    pub storage: Storage,
    pub coordinator: Arc<BuildCoordinator>,
}
