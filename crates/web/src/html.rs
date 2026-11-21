//! HTML to readable lines. Ported from v1 (src/main/webpage.js) with its fixes: tables become "cell | cell"
//! rows, tags with ">" inside quoted attributes are handled, and query filtering puts matching table rows first.

use regex::Regex;
use std::sync::LazyLock;

/// A tag, allowing ">" inside quoted attribute values.
const TAG: &str = r#"<(?:[^>"']|"[^"]*"|'[^']*')*>"#;

static TAG_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(TAG).unwrap());
static COMMENT_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?s)<!--.*?-->").unwrap());
static HEAD_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?is)<head\b.*?</head>").unwrap());
static NOISE_RES: LazyLock<Vec<Regex>> = LazyLock::new(|| {
    ["script", "style", "noscript", "svg", "template", "iframe", "nav", "footer", "aside", "form"]
        .iter()
        .map(|tag| Regex::new(&format!(r"(?is)<{tag}\b.*?</{tag}\s*>")).unwrap())
        .collect()
});
static ROW_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"(?is)<tr\b(?:[^>"']|"[^"]*"|'[^']*')*>(.*?)</tr\s*>"#).unwrap());
static CELL_END_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)</t[dh]\s*>").unwrap());
static HEADING_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)<h[1-6]\b[^>]*>").unwrap());
static LI_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)<li\b[^>]*>").unwrap());
static BR_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)<br\s*/?>").unwrap());
static BLOCK_END_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)</(?:p|div|section|article|li|h[1-6]|ul|ol|table|tbody|thead|blockquote|pre|dd|dt)\s*>").unwrap());
static ENTITY_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"&(#[xX][0-9a-fA-F]+|#[0-9]+|[a-zA-Z]+);").unwrap());
static TITLE_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?is)<title[^>]*>(.*?)</title>").unwrap());
static SPACES_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[ \t\x0C\x0B\u{00a0}]+").unwrap());

pub fn decode_entities(text: &str) -> String {
    ENTITY_RE
        .replace_all(text, |caps: &regex::Captures<'_>| {
            let body = &caps[1];
            let decoded = if let Some(hex) = body.strip_prefix("#x").or_else(|| body.strip_prefix("#X")) {
                u32::from_str_radix(hex, 16).ok().and_then(char::from_u32).map(String::from)
            } else if let Some(dec) = body.strip_prefix('#') {
                dec.parse::<u32>().ok().and_then(char::from_u32).map(String::from)
            } else {
                named_entity(&body.to_ascii_lowercase()).map(str::to_string)
            };
            decoded.unwrap_or_else(|| caps[0].to_string())
        })
        .into_owned()
}

fn named_entity(name: &str) -> Option<&'static str> {
    Some(match name {
        "amp" => "&",
        "quot" => "\"",
        "apos" => "'",
        "lt" => "<",
        "gt" => ">",
        "nbsp" => " ",
        "zwnj" => "\u{200c}",
        "zwj" => "\u{200d}",
        "laquo" => "«",
        "raquo" => "»",
        "ndash" | "mdash" => "-",
        "hellip" => "...",
        "copy" => "(c)",
        "reg" => "(R)",
        "times" => "x",
        _ => return None,
    })
}

/// Removes all tags and decodes entities; whitespace collapsed.
pub fn strip_tags(html: &str) -> String {
    let text = decode_entities(&TAG_RE.replace_all(html, " "));
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

pub fn title(html: &str) -> String {
    TITLE_RE.captures(html).map(|c| strip_tags(&c[1])).unwrap_or_default()
}

/// HTML -> readable lines: headings ("# "), list items ("- "), paragraphs and table rows ("a | b | c").
pub fn html_to_lines(html: &str) -> Vec<String> {
    let mut s = COMMENT_RE.replace_all(html, " ").into_owned();
    for re in NOISE_RES.iter() {
        s = re.replace_all(&s, " ").into_owned();
    }
    s = HEAD_RE.replace(&s, " ").into_owned();
    s = ROW_RE
        .replace_all(&s, |caps: &regex::Captures<'_>| {
            let cells: Vec<String> = CELL_END_RE.split(&caps[1]).map(strip_tags).filter(|c| !c.is_empty()).collect();
            if cells.is_empty() { "\n".to_string() } else { format!("\n{}\n", cells.join(" | ")) }
        })
        .into_owned();
    s = HEADING_RE.replace_all(&s, "\n# ").into_owned();
    s = LI_RE.replace_all(&s, "\n- ").into_owned();
    s = BR_RE.replace_all(&s, "\n").into_owned();
    s = BLOCK_END_RE.replace_all(&s, "\n").into_owned();
    s = TAG_RE.replace_all(&s, " ").into_owned();
    s = decode_entities(&s);

    let mut lines: Vec<String> = Vec::new();
    for raw in s.split('\n') {
        let line = SPACES_RE.replace_all(raw, " ").trim().to_string();
        if line.is_empty() || line == "-" || line == "#" || lines.last() == Some(&line) {
            continue;
        }
        lines.push(line);
    }
    lines
}

/// Text folded for matching: lower case, Persian and Arabic digits as ASCII, Arabic ي/ك as Persian ی/ک, no
/// half-spaces. A Persian query ("نوت ۱۳") then finds "Note 13", and "علي" finds "علی".
fn fold(text: &str) -> String {
    text.to_lowercase()
        .chars()
        .filter(|c| *c != '\u{200c}')
        .map(|c| match c {
            '۰'..='۹' => char::from(b'0' + (c as u32 - '۰' as u32) as u8),
            '٠'..='٩' => char::from(b'0' + (c as u32 - '٠' as u32) as u8),
            'ي' | 'ى' => 'ی',
            'ك' => 'ک',
            _ => c,
        })
        .collect()
}

/// Lines mentioning the query words: matching table rows first (where values are), then other matching lines
/// with one line of context. None when nothing matches.
pub fn filter_lines(lines: &[String], query: &str) -> Option<Vec<String>> {
    let tokens: Vec<String> = fold(query)
        .split(|c: char| c.is_whitespace() || ",،;:".contains(c))
        .filter(|t| t.chars().count() > 1)
        .map(str::to_string)
        .collect();
    if tokens.is_empty() {
        return None;
    }
    let matches = |line: &str| {
        let line = fold(line);
        tokens.iter().any(|t| line.contains(t.as_str()))
    };
    let mut rows = Vec::new();
    let mut context = std::collections::BTreeSet::new();
    for (i, line) in lines.iter().enumerate() {
        if !matches(line) {
            continue;
        }
        if line.contains(" | ") {
            rows.push(line.clone());
        } else {
            context.insert(i.saturating_sub(1));
            context.insert(i);
            context.insert(i + 1);
        }
    }
    let others: Vec<String> = context.into_iter().filter(|&i| i < lines.len()).map(|i| lines[i].clone()).collect();
    match (rows.is_empty(), others.is_empty()) {
        (true, true) => None,
        (false, false) => Some(rows.into_iter().chain(std::iter::once("---".to_string())).chain(others).collect()),
        _ => Some(rows.into_iter().chain(others).collect()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PAGE: &str = r#"<html><head><title>Gheimat &amp; Bazar</title><style>.x{}</style></head><body>
        <nav>menu</nav><h1>Prices</h1>
        <table><tr class="a" title="5 > 3"><th>Item</th><th>Price</th></tr>
        <tr><td title="x > y">Gold 18k</td><td>262,073,000</td></tr><tr><td>دلار</td><td>2,695,000</td></tr></table>
        <p>Updated &#8212; today&nbsp;now &#x1F600;</p><ul><li>one</li><li>two</li></ul>
        <script>evil()</script><!-- hidden --></body></html>"#;

    #[test]
    fn page_becomes_clean_lines_with_table_rows() {
        let lines = html_to_lines(PAGE);
        assert_eq!(
            lines,
            ["# Prices", "Item | Price", "Gold 18k | 262,073,000", "دلار | 2,695,000", "Updated — today now 😀", "- one", "- two"]
        );
        assert_eq!(title(PAGE), "Gheimat & Bazar");
    }

    #[test]
    fn query_keeps_matching_rows_first() {
        let lines = html_to_lines(PAGE);
        let filtered = filter_lines(&lines, "دلار updated").unwrap();
        assert_eq!(filtered[0], "دلار | 2,695,000");
        assert_eq!(filtered[1], "---");
        assert!(filtered.contains(&"Updated — today now 😀".to_string()));
        assert!(filter_lines(&lines, "nothing-here").is_none());
        assert!(filter_lines(&lines, "a").is_none());
    }

    #[test]
    fn persian_queries_match_latin_digits_and_arabic_letters() {
        let lines = vec!["Redmi Note 13 Pro | 109,900,000".to_string(), "other".to_string(), "آقای علی | 5".to_string()];
        assert_eq!(filter_lines(&lines, "نوت ۱۳").unwrap(), vec!["Redmi Note 13 Pro | 109,900,000".to_string()]);
        assert_eq!(filter_lines(&lines, "علي").unwrap(), vec!["آقای علی | 5".to_string()]);
    }

    #[test]
    fn entities_and_tags() {
        assert_eq!(decode_entities("a&amp;b &lt;c&gt; &unknown; &#65;"), "a&b <c> &unknown; A");
        assert_eq!(strip_tags(r#"<a href="x" title="1 > 0">Hi</a>  <b>there</b>"#), "Hi there");
    }
}
