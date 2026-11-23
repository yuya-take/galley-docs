//! MCP の接続設定で更新者名を渡すヘッダー（`X-Author-Name`）の取り決め。
//!
//! Claude Code などは ASCII 以外の文字をヘッダーの値に入れられないので、日本語の名前は
//! UTF-8 をパーセントエンコードした値（`佐藤` なら `%E4%BD%90%E8%97%A4`）で送ってもらう。
//! 接続の案内（`mcp_command`）が [`encode`] で値を作り、`crates/server` の MCP が [`decode`] で戻す。

/// 更新者名を渡すヘッダーの名前。
pub const AUTHOR_HEADER: &str = "X-Author-Name";

/// ヘッダーに入れる値にする。印字できる ASCII だけで `%` を含まない名前はそのまま、
/// それ以外は英数字と `-_.~` を除いて UTF-8 のバイトを `%XX` にする（`%` を必ずエンコードするので
/// [`decode`] で元に戻る）。
pub fn encode(name: &str) -> String {
    let is_plain = |c: char| c.is_ascii_graphic() || c == ' ';
    if name.chars().all(is_plain) && !name.contains('%') {
        return name.to_owned();
    }
    let mut encoded = String::with_capacity(name.len() * 3);
    for byte in name.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
            encoded.push(char::from(byte));
        } else {
            encoded.push_str(&format!("%{byte:02X}"));
        }
    }
    encoded
}

/// ヘッダーの値から名前を読む。パーセントエンコードした値は戻し、戻せない値（`%` の後が16進でない、
/// 戻すと UTF-8 にならない）はそのまま使う。ほかのクライアントが送る生の UTF-8 の値もそのまま読める。
/// UTF-8 として読めない値は `None`。前後の空白は除く（名前としての検査は呼び出し側の [`AuthorName`] で行う）。
///
/// [`AuthorName`]: galley_core::domain::revision::AuthorName
pub fn decode(value: &[u8]) -> Option<String> {
    let raw = std::str::from_utf8(value).ok()?;
    let name = percent_decode(raw).unwrap_or_else(|| raw.to_owned());
    Some(name.trim().to_owned())
}

/// `%XX` を戻す（`encodeURIComponent` や [`encode`] で作った値）。16進でない `%` があるか、
/// UTF-8 として読めなければ `None`。
pub(crate) fn percent_decode(value: &str) -> Option<String> {
    let bytes = value.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let hex = std::str::from_utf8(bytes.get(i + 1..i + 3)?).ok()?;
            if !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
                return None;
            }
            decoded.push(u8::from_str_radix(hex, 16).ok()?);
            i += 3;
        } else {
            decoded.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(decoded).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encodes_non_ascii_names() {
        assert_eq!(encode("佐藤"), "%E4%BD%90%E8%97%A4");
        assert_eq!(
            encode("佐藤 太郎"),
            "%E4%BD%90%E8%97%A4%20%E5%A4%AA%E9%83%8E"
        );
    }

    #[test]
    fn keeps_plain_ascii_names() {
        assert_eq!(encode("Sato"), "Sato");
        assert_eq!(encode("Taro Sato"), "Taro Sato");
        assert_eq!(encode(r#"a"$(rm -rf ~)"#), r#"a"$(rm -rf ~)"#);
    }

    #[test]
    fn always_encodes_percent_and_controls() {
        assert_eq!(encode("50%off"), "50%25off");
        assert_eq!(encode("100%25"), "100%2525");
        assert_eq!(encode("a\tb"), "a%09b");
    }

    #[test]
    fn decodes_encoded_raw_and_ascii_names_alike() {
        assert_eq!(decode(b"%E4%BD%90%E8%97%A4").as_deref(), Some("佐藤"));
        assert_eq!(decode("佐藤".as_bytes()).as_deref(), Some("佐藤"));
        assert_eq!(decode(b"Sato").as_deref(), Some("Sato"));
        assert_eq!(decode(b"  %e4%bd%90%e8%97%a4 ").as_deref(), Some("佐藤"));
    }

    #[test]
    fn reads_percent_in_ascii_names() {
        // `%` の後が16進でなければ、エンコードしていない値として読む
        assert_eq!(decode(b"50%off").as_deref(), Some("50%off"));
        assert_eq!(decode(b"100%").as_deref(), Some("100%"));
        // 16進が続けば戻す（`100%25` という名前は、案内では `100%2525` になる）
        assert_eq!(decode(b"100%25").as_deref(), Some("100%"));
        // 戻すと UTF-8 にならない値も、そのまま読む
        assert_eq!(decode(b"%E4%BD").as_deref(), Some("%E4%BD"));
        // 符号付きに見える16進（`%+1`）は16進として扱わない
        assert_eq!(decode(b"a%+1").as_deref(), Some("a%+1"));
    }

    #[test]
    fn rejects_values_that_are_not_utf8() {
        assert_eq!(decode(&[0xE4, 0xBD]), None);
    }

    #[test]
    fn encoded_names_round_trip() {
        for name in [
            "佐藤",
            "佐藤 太郎",
            "Sato",
            "50%off",
            "100%25",
            "100%",
            "%E4%BD%90",
            "O'Brien!",
            "a\"b\\c",
        ] {
            let encoded = encode(name);
            assert!(encoded.is_ascii(), "{encoded}");
            assert_eq!(
                decode(encoded.as_bytes()).as_deref(),
                Some(name),
                "{encoded}"
            );
        }
    }
}
