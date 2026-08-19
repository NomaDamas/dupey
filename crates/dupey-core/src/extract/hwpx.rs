//! hwpx extraction (stub; implemented in the hwpx slice).

use std::path::Path;

use super::CanonicalText;
use crate::error::{Error, Result};

pub(crate) fn extract_hwpx(path: &Path) -> Result<CanonicalText> {
    Err(Error::Extract {
        path: path.to_path_buf(),
        message: "hwpx extract not implemented".into(),
    })
}
