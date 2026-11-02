//! `db.query.text` sanitization: dialect-aware, and fail-closed on anything unbalanced.

use autotel::{TraceCollector, database, sanitize_query};
use serde_json::json;

#[test]
fn sanitize_query_strips_literals_and_keeps_structure() {
    let cases = [
        (
            "SELECT * FROM users WHERE email = 'a@b.com' AND id = 42",
            "SELECT * FROM users WHERE email = ? AND id = ?",
        ),
        (
            "UPDATE t SET note = 'it''s here' WHERE id = 7",
            "UPDATE t SET note = ? WHERE id = ?",
        ),
        (
            "SELECT \"Order\".total FROM \"Order\" WHERE price > 9.99 LIMIT 10",
            "SELECT \"Order\".total FROM \"Order\" WHERE price > ? LIMIT ?",
        ),
        (
            "SELECT reserve_stock($1, $2)",
            "SELECT reserve_stock($1, $2)",
        ),
        (
            "SELECT * FROM t2 WHERE c = :name OR c = ?",
            "SELECT * FROM t2 WHERE c = :name OR c = ?",
        ),
        (
            "SELECT 1 -- secret 'token'\nFROM t /* user 99 */ WHERE x IN (1, 2, 3)",
            "SELECT ? FROM t WHERE x IN (?, ?, ?)",
        ),
        (
            "INSERT INTO t VALUES (0x1F, -5, 1e3)",
            "INSERT INTO t VALUES (?, -?, ?)",
        ),
    ];
    for (input, expected) in cases {
        assert_eq!(
            sanitize_query(input, "postgresql"),
            expected,
            "input: {input}"
        );
    }
}

#[test]
fn mysql_rules_backslash_escapes_double_quoted_strings_and_hash_comments() {
    let cases = [
        // Without backslash escapes, `\'` would close the string and expose `secret`.
        (
            r"SELECT * FROM u WHERE pw = 'a\' OR name = secret' AND id = 1",
            "SELECT * FROM u WHERE pw = ? AND id = ?",
        ),
        // In MySQL "…" is a string, not an identifier.
        (
            "SELECT * FROM u WHERE email = \"jo@example.com\"",
            "SELECT * FROM u WHERE email = ?",
        ),
        (
            "SELECT `order`.id FROM `order`",
            "SELECT `order`.id FROM `order`",
        ),
        ("SELECT 1 # token='abc'\nFROM t", "SELECT ? FROM t"),
    ];
    for (input, expected) in cases {
        assert_eq!(sanitize_query(input, "mysql"), expected, "input: {input}");
    }
}

#[test]
fn postgres_rules_escape_strings_and_dollar_quotes() {
    let cases = [
        (r"SELECT E'it\'s a secret' AS note", "SELECT ? AS note"),
        (
            "SELECT $$ssn 123-45-6789$$, $tag$card 4111 'x'$tag$ FROM t",
            "SELECT ?, ? FROM t",
        ),
        // `$1` is a placeholder; `a$b` is an identifier.
        (
            "SELECT a$b FROM t WHERE id = $1",
            "SELECT a$b FROM t WHERE id = $1",
        ),
    ];
    for (input, expected) in cases {
        assert_eq!(
            sanitize_query(input, "postgresql"),
            expected,
            "input: {input}"
        );
    }
}

#[test]
fn unknown_dialects_treat_double_quotes_as_strings() {
    assert_eq!(
        sanitize_query("SELECT * FROM u WHERE email = \"jo@example.com\"", ""),
        "SELECT * FROM u WHERE email = ?"
    );
    // SQLite reads `"…"` as a string when no column has that name.
    assert_eq!(
        sanitize_query("SELECT * FROM u WHERE email = \"jo@example.com\"", "sqlite"),
        "SELECT * FROM u WHERE email = ?"
    );
}

#[test]
fn unterminated_literals_fail_closed() {
    for (system, input) in [
        (
            "postgresql",
            "SELECT * FROM t WHERE a = 'never closed secret",
        ),
        ("postgresql", "SELECT $$ never closed secret"),
        ("postgresql", "SELECT \"never closed secret"),
        (
            "mysql",
            r"SELECT * FROM t WHERE a = 'ends in backslash secret\'",
        ),
        ("mysql", "SELECT `never closed secret"),
    ] {
        let sanitized = sanitize_query(input, system);
        assert!(
            !sanitized.contains("secret"),
            "{system}: {input} -> {sanitized}"
        );
        assert!(sanitized.ends_with('?'), "{system}: {input} -> {sanitized}");
    }
}

#[tokio::test(flavor = "current_thread")]
async fn set_query_text_uses_the_span_dialect() {
    let collector = TraceCollector::new("database").unwrap();
    database("mysql", "SELECT", "shop", "users", |ctx| async move {
        ctx.set_query_text("SELECT id FROM users WHERE email = \"jo@example.com\"");
    })
    .await;
    let span = collector.expect_span("SELECT users");
    assert_eq!(
        span.attributes["db.query.text"],
        json!("SELECT id FROM users WHERE email = ?")
    );
}
