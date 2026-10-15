//! Explorer integration (the "Packrat ▸" context menu and file associations). Filled in by 0.3.0.

use crate::installer::Env;
use std::path::Path;

pub fn register_if_enabled(_env: &Env, _exe: &Path) {}

pub fn unregister(_env: &Env) {}
