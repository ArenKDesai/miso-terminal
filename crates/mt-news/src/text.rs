//! Feed fields as plain text. Titles and summaries often carry HTML (escaped
//! or in CDATA) and HTML entities; the terminal shows neither.

/// Tags removed, entities decoded, whitespace collapsed to single spaces.
pub fn plain(s: &str) -> String {
    let stripped = strip_tags(s);
    let decoded = decode_entities(&stripped);
    decoded.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Remove `<tag …>`, `</tag>` and `<!-- … -->`. A `<` that does not start a
/// tag (`S&P < 5,000`) stays. Block-level tags become a space so words on
/// either side do not run together.
fn strip_tags(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find('<') {
        out.push_str(&rest[..i]);
        let tail = &rest[i..];
        let starts_tag = tail[1..]
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_alphabetic() || matches!(c, '/' | '!' | '?'));
        if !starts_tag {
            out.push('<');
            rest = &tail[1..];
            continue;
        }
        let end = if tail.starts_with("<!--") {
            tail.find("-->").map(|j| j + 3)
        } else {
            tail.find('>').map(|j| j + 1)
        };
        match end {
            Some(j) => {
                out.push(' ');
                rest = &tail[j..];
            }
            // An unclosed tag: drop the rest rather than show markup.
            None => {
                rest = "";
            }
        }
    }
    out.push_str(rest);
    out
}

/// Decode `&amp;`, `&#8217;`, `&#x2019;` and the HTML entities feeds use.
/// Unknown entities are left as written.
pub fn decode_entities(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find('&') {
        out.push_str(&rest[..i]);
        let tail = &rest[i..];
        // Entities are short; a stray `&` (AT&T) is not one.
        let semi = tail[1..].find(';').filter(|&j| j > 0 && j <= 10);
        let decoded = semi.and_then(|j| entity(&tail[1..=j]).map(|c| (c, j + 2)));
        match decoded {
            Some((c, len)) => {
                out.push(c);
                rest = &tail[len..];
            }
            None => {
                out.push('&');
                rest = &tail[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

fn entity(name: &str) -> Option<char> {
    if let Some(num) = name.strip_prefix('#') {
        let code = match num.strip_prefix(['x', 'X']) {
            Some(hex) => u32::from_str_radix(hex, 16).ok()?,
            None => num.parse().ok()?,
        };
        return char::from_u32(code).filter(|&c| c != '\0');
    }
    Some(match name {
        "amp" => '&',
        "lt" => '<',
        "gt" => '>',
        "quot" => '"',
        "apos" => '\'',
        "nbsp" => ' ',
        "ndash" => '–',
        "mdash" => '—',
        "lsquo" => '‘',
        "rsquo" => '’',
        "sbquo" => '‚',
        "ldquo" => '“',
        "rdquo" => '”',
        "bdquo" => '„',
        "hellip" => '…',
        "bull" => '•',
        "middot" => '·',
        "laquo" => '«',
        "raquo" => '»',
        "copy" => '©',
        "reg" => '®',
        "trade" => '™',
        "deg" => '°',
        "euro" => '€',
        "pound" => '£',
        "yen" => '¥',
        "cent" => '¢',
        "times" => '×',
        "divide" => '÷',
        "plusmn" => '±',
        "minus" => '−',
        "frac12" => '½',
        "frac14" => '¼',
        "frac34" => '¾',
        "eacute" => 'é',
        "egrave" => 'è',
        "aacute" => 'á',
        "agrave" => 'à',
        "iacute" => 'í',
        "oacute" => 'ó',
        "uacute" => 'ú',
        "ntilde" => 'ñ',
        "ccedil" => 'ç',
        "auml" => 'ä',
        "ouml" => 'ö',
        "uuml" => 'ü',
        "szlig" => 'ß',
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn html_becomes_text() {
        assert_eq!(
            plain("<p>Prices <b>rose</b>&nbsp;3%.</p><p>Gas &amp; power&#8217;s day</p>"),
            "Prices rose 3%. Gas & power’s day"
        );
        assert_eq!(plain("AT&T and S&P < 5,000"), "AT&T and S&P < 5,000");
        assert_eq!(plain("a<!-- hidden <b> -->b"), "a b");
        assert_eq!(
            plain("Fed&#x2019;s cut &unknown; here"),
            "Fed’s cut &unknown; here"
        );
        assert_eq!(plain("  spaced\n\tout  "), "spaced out");
        assert_eq!(plain("broken <a href="), "broken");
    }
}
