use std::sync::Arc;

use super::load_current;
use crate::{
    app::error::AppError,
    domain::{
        document::Document,
        revision::{Revision, RevisionNumber, RevisionRepository},
        shared::RevisionId,
    },
};

/// 資料の版を探す。番号を指定しなければ現在版（最新）。
pub struct FindRevision {
    revisions: Arc<dyn RevisionRepository>,
}

impl FindRevision {
    pub fn new(revisions: Arc<dyn RevisionRepository>) -> Self {
        Self { revisions }
    }

    pub async fn execute(
        &self,
        document: &Document,
        number: Option<u32>,
    ) -> Result<Revision, AppError> {
        let Some(number) = number else {
            return load_current(self.revisions.as_ref(), document).await;
        };
        let number = RevisionNumber::new(number).map_err(|_| AppError::RevisionNotFound)?;
        self.revisions
            .find_by_number(document.id, number)
            .await?
            .ok_or(AppError::RevisionNotFound)
    }
}

/// ID から版を探す（アーカイブした資料の版も返す）。資料配信が、前の形の URL（`/r/<revision_id>`）を
/// 実体のハッシュの URL に振り向けるのに使う。
pub struct FindRevisionById {
    revisions: Arc<dyn RevisionRepository>,
}

impl FindRevisionById {
    pub fn new(revisions: Arc<dyn RevisionRepository>) -> Self {
        Self { revisions }
    }

    pub async fn execute(&self, id: RevisionId) -> Result<Revision, AppError> {
        self.revisions
            .find_by_id(id)
            .await?
            .ok_or(AppError::RevisionNotFound)
    }
}
