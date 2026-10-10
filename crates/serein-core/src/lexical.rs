//! Exact tokenization through the Unicode tokenizer shipped with this SQLite connection.
//!
//! Keeping the tokenizer handle tied to its connection avoids drifting from the tokenizer
//! that backs Serein's FTS5 index when SQLite's Unicode tables or implementation change.

use std::ffi::{c_char, c_int, c_void, CStr};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::ptr;
use std::slice;
use std::str;

use rusqlite::{ffi, Connection};

use crate::{Error, Result};

const FTS5_API_POINTER: &[u8] = b"fts5_api_ptr\0";
const FTS5_API_QUERY: &[u8] = b"SELECT fts5(?1)\0";
const UNICODE61_NAME: &[u8] = b"unicode61\0";

/// A handle to the same default `unicode61` tokenizer used by FTS5 on `connection`.
///
/// FTS5 owns the tokenizer implementation. This wrapper owns only the tokenizer instance
/// and a copy of its function table, and keeps the originating connection alive until the
/// instance is deleted.
pub(crate) struct Unicode61<'connection> {
    _connection: &'connection Connection,
    tokenizer: *mut ffi::Fts5Tokenizer,
    delete: unsafe extern "C" fn(*mut ffi::Fts5Tokenizer),
    tokenize: unsafe extern "C" fn(
        *mut ffi::Fts5Tokenizer,
        *mut c_void,
        c_int,
        *const c_char,
        c_int,
        Option<
            unsafe extern "C" fn(*mut c_void, c_int, *const c_char, c_int, c_int, c_int) -> c_int,
        >,
    ) -> c_int,
}

impl<'connection> Unicode61<'connection> {
    /// Instantiate the connection's built-in FTS5 `unicode61` tokenizer with default options.
    pub(crate) fn new(connection: &'connection Connection) -> Result<Self> {
        let api = fts5_api(connection)?;
        if api.is_null() {
            return Err(Error(
                "DB_ERROR",
                "The bundled SQLite FTS5 API is unavailable.".into(),
            ));
        }

        let mut user_data = ptr::null_mut();
        let mut tokenizer_api = unsafe { std::mem::zeroed::<ffi::fts5_tokenizer>() };
        let find = unsafe { (*api).xFindTokenizer }.ok_or_else(|| {
            Error(
                "DB_ERROR",
                "The bundled SQLite FTS5 tokenizer lookup is unavailable.".into(),
            )
        })?;
        let rc = unsafe {
            find(
                api,
                UNICODE61_NAME.as_ptr().cast(),
                &mut user_data,
                &mut tokenizer_api,
            )
        };
        if rc != ffi::SQLITE_OK {
            return Err(sqlite_error(
                connection,
                rc,
                "Could not find SQLite's unicode61 tokenizer",
            ));
        }

        let create = tokenizer_api.xCreate.ok_or_else(|| {
            Error(
                "DB_ERROR",
                "The bundled SQLite unicode61 tokenizer cannot be created.".into(),
            )
        })?;
        let delete = tokenizer_api.xDelete.ok_or_else(|| {
            Error(
                "DB_ERROR",
                "The bundled SQLite unicode61 tokenizer cannot be deleted safely.".into(),
            )
        })?;
        let tokenize = tokenizer_api.xTokenize.ok_or_else(|| {
            Error(
                "DB_ERROR",
                "The bundled SQLite unicode61 tokenizer cannot tokenize text.".into(),
            )
        })?;

        let mut tokenizer = ptr::null_mut();
        // Zero arguments select the same unicode61 defaults as `tokenize='unicode61'` in
        // the existing atom_fts table.
        let rc = unsafe { create(user_data, ptr::null_mut(), 0, &mut tokenizer) };
        if rc != ffi::SQLITE_OK || tokenizer.is_null() {
            if !tokenizer.is_null() {
                unsafe { delete(tokenizer) };
            }
            return Err(if rc == ffi::SQLITE_OK {
                Error(
                    "DB_ERROR",
                    "SQLite returned an empty unicode61 tokenizer handle.".into(),
                )
            } else {
                sqlite_error(
                    connection,
                    rc,
                    "Could not create SQLite's unicode61 tokenizer",
                )
            });
        }

        Ok(Self {
            _connection: connection,
            tokenizer,
            delete,
            tokenize,
        })
    }

    /// Return the normalized FTS5 terms emitted for one document by `unicode61`.
    pub(crate) fn tokenize(&self, text: &str) -> Result<Vec<String>> {
        if text.is_empty() {
            return Ok(Vec::new());
        }
        let byte_len = c_int::try_from(text.len()).map_err(|_| {
            Error(
                "INVALID_REQUEST",
                "Text is too large for SQLite tokenization.".into(),
            )
        })?;

        let mut sink = TokenSink::default();
        let rc = unsafe {
            (self.tokenize)(
                self.tokenizer,
                (&mut sink as *mut TokenSink).cast(),
                ffi::FTS5_TOKENIZE_DOCUMENT,
                text.as_ptr().cast(),
                byte_len,
                Some(collect_token),
            )
        };
        if rc != ffi::SQLITE_OK {
            return Err(if sink.callback_failed {
                Error(
                    "DB_ERROR",
                    "SQLite's unicode61 tokenizer returned invalid token data.".into(),
                )
            } else {
                Error(
                    "DB_ERROR",
                    format!("SQLite's unicode61 tokenizer failed (SQLite {rc})."),
                )
            });
        }
        Ok(sink.tokens)
    }
}

impl Drop for Unicode61<'_> {
    fn drop(&mut self) {
        if !self.tokenizer.is_null() {
            unsafe { (self.delete)(self.tokenizer) };
            self.tokenizer = ptr::null_mut();
        }
    }
}

#[derive(Default)]
struct TokenSink {
    tokens: Vec<String>,
    callback_failed: bool,
}

/// Copy one FTS5 token into Rust-owned memory. Every Rust operation that can unwind is
/// contained here because unwinding through SQLite's C callback frame is undefined behavior.
unsafe extern "C" fn collect_token(
    context: *mut c_void,
    _token_flags: c_int,
    token: *const c_char,
    token_len: c_int,
    _start: c_int,
    _end: c_int,
) -> c_int {
    if context.is_null() || token_len < 0 || (token.is_null() && token_len != 0) {
        return ffi::SQLITE_MISUSE;
    }

    let sink = unsafe { &mut *context.cast::<TokenSink>() };
    let copied = catch_unwind(AssertUnwindSafe(|| -> std::result::Result<(), ()> {
        let length = usize::try_from(token_len).map_err(|_| ())?;
        let bytes = if length == 0 {
            &[][..]
        } else {
            unsafe { slice::from_raw_parts(token.cast::<u8>(), length) }
        };
        let value = str::from_utf8(bytes).map_err(|_| ())?;
        sink.tokens.push(value.to_owned());
        Ok(())
    }));

    match copied {
        Ok(Ok(())) => ffi::SQLITE_OK,
        Ok(Err(())) | Err(_) => {
            sink.callback_failed = true;
            ffi::SQLITE_ERROR
        }
    }
}

fn fts5_api(connection: &Connection) -> Result<*mut ffi::fts5_api> {
    let db = unsafe { connection.handle() };
    let mut statement = ptr::null_mut();
    let prepare_rc = unsafe {
        ffi::sqlite3_prepare_v2(
            db,
            FTS5_API_QUERY.as_ptr().cast(),
            -1,
            &mut statement,
            ptr::null_mut(),
        )
    };
    let mut statement = RawStatement(statement);
    if prepare_rc != ffi::SQLITE_OK {
        return Err(sqlite_error(
            connection,
            prepare_rc,
            "Could not prepare the FTS5 API lookup",
        ));
    }
    if statement.0.is_null() {
        return Err(Error(
            "DB_ERROR",
            "SQLite returned an empty FTS5 API lookup statement.".into(),
        ));
    }

    let mut api = ptr::null_mut::<ffi::fts5_api>();
    let bind_rc = unsafe {
        ffi::sqlite3_bind_pointer(
            statement.0,
            1,
            (&mut api as *mut *mut ffi::fts5_api).cast(),
            FTS5_API_POINTER.as_ptr().cast(),
            None,
        )
    };
    if bind_rc != ffi::SQLITE_OK {
        return Err(sqlite_error(
            connection,
            bind_rc,
            "Could not bind the FTS5 API pointer",
        ));
    }

    let step_rc = unsafe { ffi::sqlite3_step(statement.0) };
    if step_rc != ffi::SQLITE_ROW {
        return Err(sqlite_error(
            connection,
            step_rc,
            "Could not retrieve the FTS5 API pointer",
        ));
    }
    let finalize_rc = statement.finalize();
    if finalize_rc != ffi::SQLITE_OK {
        return Err(sqlite_error(
            connection,
            finalize_rc,
            "Could not finalize the FTS5 API lookup",
        ));
    }
    if api.is_null() {
        return Err(Error(
            "DB_ERROR",
            "SQLite did not provide its FTS5 API pointer.".into(),
        ));
    }
    Ok(api)
}

struct RawStatement(*mut ffi::sqlite3_stmt);

impl RawStatement {
    fn finalize(&mut self) -> c_int {
        if self.0.is_null() {
            return ffi::SQLITE_OK;
        }
        let statement = std::mem::replace(&mut self.0, ptr::null_mut());
        unsafe { ffi::sqlite3_finalize(statement) }
    }
}

impl Drop for RawStatement {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe { ffi::sqlite3_finalize(self.0) };
            self.0 = ptr::null_mut();
        }
    }
}

fn sqlite_error(connection: &Connection, rc: c_int, context: &str) -> Error {
    let db = unsafe { connection.handle() };
    let detail = unsafe {
        let message = ffi::sqlite3_errmsg(db);
        if message.is_null() {
            ffi::sqlite3_errstr(rc)
        } else {
            message
        }
    };
    let detail = if detail.is_null() {
        format!("SQLite error {rc}")
    } else {
        unsafe { CStr::from_ptr(detail) }
            .to_string_lossy()
            .into_owned()
    };
    Error::from(rusqlite::Error::SqliteFailure(
        ffi::Error::new(rc),
        Some(format!("{context}: {detail}")),
    ))
}

#[cfg(test)]
mod tests {
    use super::Unicode61;
    use rusqlite::{params, Connection};

    fn fixture_connection() -> Connection {
        let connection = Connection::open_in_memory().unwrap();
        connection
            .execute_batch(
                "CREATE VIRTUAL TABLE token_docs USING fts5(content, tokenize='unicode61');\
                 CREATE VIRTUAL TABLE token_vocab USING fts5vocab(token_docs, 'instance');",
            )
            .unwrap();
        connection
    }

    fn insert_document(connection: &Connection, rowid: i64, text: &str) {
        connection
            .execute(
                "INSERT INTO token_docs(rowid, content) VALUES (?1, ?2)",
                params![rowid, text],
            )
            .unwrap();
    }

    fn vocab_tokens(connection: &Connection, rowid: i64) -> Vec<String> {
        let mut statement = connection
            .prepare("SELECT term FROM token_vocab WHERE doc=?1 ORDER BY offset")
            .unwrap();
        statement
            .query_map([rowid], |row| row.get(0))
            .unwrap()
            .map(Result::unwrap)
            .collect()
    }

    fn matching_rows(connection: &Connection, query: &str) -> Vec<i64> {
        let mut statement = connection
            .prepare("SELECT rowid FROM token_docs WHERE token_docs MATCH ?1 ORDER BY rowid")
            .unwrap();
        statement
            .query_map([query], |row| row.get(0))
            .unwrap()
            .map(Result::unwrap)
            .collect()
    }

    fn match_term(term: &str) -> String {
        format!("\"{}\"", term.replace('"', "\"\""))
    }

    #[test]
    fn emitted_terms_equal_the_terms_indexed_by_unicode61_and_match_them() {
        let connection = fixture_connection();
        let tokenizer = Unicode61::new(&connection).unwrap();
        let examples = [
            "J501 501; 33 cm versus 30cm",
            "Café cafe\u{0301}",
            "ộ o\u{0323}\u{0302} a\u{0301}\u{0327}",
            "English中文 and 日本語 MixedLatin漢字",
        ];

        for (index, text) in examples.iter().enumerate() {
            let rowid = index as i64 + 1;
            insert_document(&connection, rowid, text);
            let emitted = tokenizer.tokenize(text).unwrap();
            assert_eq!(emitted, vocab_tokens(&connection, rowid), "input: {text:?}");
            for term in emitted {
                assert_eq!(matching_rows(&connection, &match_term(&term)), vec![rowid]);
            }
        }
    }

    #[test]
    fn match_observes_full_unicode61_word_boundaries_and_short_identifiers() {
        let connection = fixture_connection();
        let tokenizer = Unicode61::new(&connection).unwrap();
        insert_document(&connection, 1, "J501");
        insert_document(&connection, 2, "501");
        insert_document(&connection, 3, "33 cm");
        insert_document(&connection, 4, "30cm");
        insert_document(&connection, 5, "English中文");
        insert_document(&connection, 6, "中文");

        assert_eq!(tokenizer.tokenize("J501").unwrap(), vec!["j501"]);
        assert_eq!(tokenizer.tokenize("501").unwrap(), vec!["501"]);
        assert_eq!(matching_rows(&connection, "\"j501\""), vec![1]);
        assert_eq!(matching_rows(&connection, "\"501\""), vec![2]);
        assert_eq!(matching_rows(&connection, "\"33\""), vec![3]);
        assert_eq!(matching_rows(&connection, "\"30cm\""), vec![4]);
        assert_eq!(matching_rows(&connection, "\"english中文\""), vec![5]);
        assert_eq!(matching_rows(&connection, "\"中文\""), vec![6]);
        assert!(matching_rows(&connection, "\"j50\"").is_empty());
        assert!(matching_rows(&connection, "\"30\"").is_empty());
        assert!(matching_rows(&connection, "\"文\"").is_empty());
    }

    #[test]
    fn composed_decomposed_and_multiple_diacritics_follow_fts5_match_behavior() {
        let connection = fixture_connection();
        let tokenizer = Unicode61::new(&connection).unwrap();
        insert_document(&connection, 1, "Café");
        insert_document(&connection, 2, "cafe\u{0301}");
        insert_document(&connection, 3, "ộ");
        insert_document(&connection, 4, "o\u{0323}\u{0302}");

        for rowid in 1..=4 {
            let input: String = connection
                .query_row(
                    "SELECT content FROM token_docs WHERE rowid=?1",
                    [rowid],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(
                tokenizer.tokenize(&input).unwrap(),
                vocab_tokens(&connection, rowid)
            );
        }

        let cafe = tokenizer.tokenize("Café").unwrap();
        assert_eq!(cafe, tokenizer.tokenize("cafe\u{0301}").unwrap());
        assert_eq!(
            matching_rows(&connection, &match_term(&cafe[0])),
            vec![1, 2]
        );

        for rowid in [3, 4] {
            for term in tokenizer
                .tokenize(
                    &connection
                        .query_row(
                            "SELECT content FROM token_docs WHERE rowid=?1",
                            [rowid],
                            |row| row.get::<_, String>(0),
                        )
                        .unwrap(),
                )
                .unwrap()
            {
                assert_eq!(matching_rows(&connection, &match_term(&term)), vec![rowid]);
            }
        }
    }

    #[test]
    fn empty_input_emits_no_terms() {
        let connection = Connection::open_in_memory().unwrap();
        let tokenizer = Unicode61::new(&connection).unwrap();
        assert!(tokenizer.tokenize("").unwrap().is_empty());
    }
}
