//! pdf extraction (stub; implemented in the pdf slice).

use std::path::Path;

use super::CanonicalText;
use crate::error::{Error, Result};

pub(crate) fn extract_pdf(path: &Path) -> Result<CanonicalText> {
    Err(Error::Extract {
        path: path.to_path_buf(),
        message: "pdf extract not implemented".into(),
    })
}
