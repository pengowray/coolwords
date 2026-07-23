//! Book-level tags: a library-organising layer above the per-word tags in
//! `user.db`. Books get named tags (some hand-applied, some derived from the
//! catalog's subjects/bookshelves — those are `auto`) so the books page can filter
//! and group a growing library.
//!
//! Shared types only for now; the server fns and the tag-editing UI arrive with the
//! page work. As in `catalog`, these must stay off `cfg(ssr)` — the client
//! deserializes them.

use serde::{Deserialize, Serialize};

/// A book tag, with the count of books carrying it (for the sidebar's tag list).
#[derive(Clone, Debug, Serialize, Deserialize, Default)]
pub struct BookTag {
    pub name: String,
    /// Derived from catalog subjects rather than applied by hand — shown muted and
    /// safe to regenerate wholesale on a re-sync.
    pub auto: bool,
    pub n_books: i64,
}

/// A book plus its tags, as rendered by the books page.
#[derive(Clone, Debug, Serialize, Deserialize, Default)]
pub struct BookWithTags {
    pub book_id: i64,
    pub slug: String,
    pub title: String,
    #[serde(default)]
    pub tags: Vec<BookTag>,
}
