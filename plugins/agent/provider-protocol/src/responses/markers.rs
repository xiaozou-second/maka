/*
 * Licensed to the Apache Software Foundation (ASF) under one
 * or more contributor license agreements.  See the NOTICE file
 * distributed with this work for additional information
 * regarding copyright ownership.  The ASF licenses this file
 * to you under the Apache License, Version 2.0 (the
 * "License"); you may not use this file except in compliance
 * with the License.  You may obtain a copy of the License at
 *
 *     http://www.apache.org/licenses/LICENSE-2.0
 *
 * Unless required by applicable law or agreed to in writing,
 * software distributed under the License is distributed on an
 * "AS IS" BASIS, WITHOUT WARRANTIES OR CONDITIONS OF ANY
 * KIND, either express or implied.  See the License for the
 * specific language governing permissions and limitations
 * under the License.
 */

//! Presentation cleanup for OpenAI Responses / ChatGPT Private Use Area
//! inline citation and entity markers (`U+E200`..`U+E204`).
//!
//! Raw wire text in `ModelPart::Text` is preserved verbatim so exact
//! Responses replay and citation anchor coordinates stay intact; human-facing
//! and cross-agent prose surfaces strip these protocol markers through here.

use std::borrow::Cow;

const OPEN: char = '\u{E200}';
const CLOSE: char = '\u{E201}';
const SEP: char = '\u{E202}';
const SPAN_OPEN: char = '\u{E203}';
const SPAN_CLOSE: char = '\u{E204}';

/// Strip OpenAI Private Use Area citation/metadata markers from completed prose.
pub fn strip_citation_markers(text: &str) -> Cow<'_, str> {
    strip(text, false)
}

/// Strip OpenAI Private Use Area markers from streaming prose, withholding an
/// incomplete trailing `U+E200...` token until its closing `U+E201` arrives.
pub fn strip_citation_markers_streaming(text: &str) -> Cow<'_, str> {
    strip(text, true)
}

/// Fast UTF-8 byte scan for `U+E200..=U+E204` (`[0xEE, 0x88, 0x80..=0x84]`).
pub fn has_citation_markers(text: &str) -> bool {
    let bytes = text.as_bytes();
    let mut i = 0;
    while i + 2 < bytes.len() {
        if bytes[i] == 0xEE && bytes[i + 1] == 0x88 && matches!(bytes[i + 2], 0x80..=0x84) {
            return true;
        }
        i += 1;
    }
    false
}

enum Marker {
    Remove { end: usize },
    Replace { end: usize, text: String },
}

fn is_ascii_tag(s: &str) -> bool {
    let mut bytes = s.bytes();
    let Some(first) = bytes.next() else {
        return false;
    };
    first.is_ascii_alphabetic()
        && bytes.all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-'))
}

fn is_ascii_token(s: &str) -> bool {
    !s.is_empty()
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b':' | b'-' | b'.' | b'/'))
}

fn is_horizontal_space(ch: char) -> bool {
    matches!(ch, ' ' | '\t')
}

fn is_span_delim(ch: char) -> bool {
    matches!(ch, SPAN_OPEN | SPAN_CLOSE)
}

fn is_trailing_punctuation(ch: char) -> bool {
    matches!(
        ch,
        ',' | '.'
            | ';'
            | ':'
            | '!'
            | '?'
            | ')'
            | ']'
            | '}'
            | '，'
            | '。'
            | '；'
            | '：'
            | '！'
            | '？'
            | '、'
            | '）'
            | '】'
            | '》'
            | '」'
            | '』'
    )
}

fn extract_entity_display(payload: &str) -> Option<String> {
    let json_slice = payload.split(SEP).next().unwrap_or(payload);
    let value: serde_json::Value = serde_json::from_str(json_slice).ok()?;
    let display = value.as_array()?.get(1)?.as_str()?;
    (!display.is_empty()).then(|| display.to_owned())
}

fn parse_marker(text: &str, start: usize, streaming: bool) -> Option<Marker> {
    let rest = text.get(start..)?;
    if !rest.starts_with(OPEN) {
        return None;
    }
    let after_open = start + OPEN.len_utf8();
    let tail = &text[after_open..];
    for (rel, ch) in tail.char_indices() {
        match ch {
            CLOSE => {
                let body = &tail[..rel];
                let end = after_open + rel + CLOSE.len_utf8();
                return classify_closed_body(body, end);
            }
            OPEN | '\n' | '\r' => return None,
            _ => {}
        }
    }
    // Reached EOF without closing U+E201.
    let end = text.len();
    if streaming {
        if tail.is_empty() || is_ascii_tag(tail) {
            return Some(Marker::Remove { end });
        }
        if let Some((tag, _)) = tail.split_once(SEP)
            && is_ascii_tag(tag)
        {
            return Some(Marker::Remove { end });
        }
        return None;
    }
    if let Some((tag, _)) = tail.split_once(SEP)
        && is_ascii_tag(tag)
    {
        return Some(Marker::Remove { end });
    }
    None
}

fn classify_closed_body(body: &str, end: usize) -> Option<Marker> {
    if let Some((tag, payload)) = body.split_once(SEP) {
        if !is_ascii_tag(tag) {
            return None;
        }
        if tag == "entity"
            && let Some(display) = extract_entity_display(payload)
        {
            return Some(Marker::Replace { end, text: display });
        }
        return Some(Marker::Remove { end });
    }
    is_ascii_token(body).then_some(Marker::Remove { end })
}

fn skip_span_delims(text: &str, mut pos: usize) -> usize {
    while let Some(ch) = text[pos..].chars().next() {
        if is_span_delim(ch) {
            pos += ch.len_utf8();
        } else {
            break;
        }
    }
    pos
}

fn skip_horizontal_space(text: &str, mut pos: usize) -> usize {
    while let Some(ch) = text[pos..].chars().next() {
        if is_horizontal_space(ch) {
            pos += ch.len_utf8();
        } else {
            break;
        }
    }
    pos
}

/// Starting at a confirmed `Marker::Remove`, consume all consecutive removed
/// markers separated only by horizontal spaces or `U+E203`/`U+E204` delimiters.
fn consume_removed_cluster(text: &str, mut pos: usize, streaming: bool) -> usize {
    while let Some(Marker::Remove { end }) = parse_marker(text, pos, streaming) {
        pos = end;
        let next = skip_span_delims(
            text,
            skip_horizontal_space(text, skip_span_delims(text, pos)),
        );
        if matches!(
            parse_marker(text, next, streaming),
            Some(Marker::Remove { .. })
        ) {
            pos = next;
        } else {
            break;
        }
    }
    pos
}

fn strip(text: &str, streaming: bool) -> Cow<'_, str> {
    if !has_citation_markers(text) {
        return Cow::Borrowed(text);
    }

    let mut out = String::with_capacity(text.len());
    let mut pos = 0;
    let mut at_line_start = true;
    let mut changed = false;

    while pos < text.len() {
        let ch = text[pos..].chars().next().expect("valid char boundary");
        if is_span_delim(ch) {
            pos += ch.len_utf8();
            changed = true;
            continue;
        }

        if ch == OPEN
            && let Some(marker) = parse_marker(text, pos, streaming)
        {
            changed = true;
            match marker {
                Marker::Replace { end, text: display } => {
                    out.push_str(&display);
                    if display.chars().any(|c| !is_horizontal_space(c)) {
                        at_line_start = display.ends_with('\n') || display.ends_with('\r');
                    }
                    pos = end;
                }
                Marker::Remove { .. } => {
                    pos = consume_removed_cluster(text, pos, streaming);
                }
            }
            continue;
        }

        if is_horizontal_space(ch) && !at_line_start {
            let space_end = skip_horizontal_space(text, pos);
            let after_delims = skip_span_delims(text, space_end);
            if matches!(
                parse_marker(text, after_delims, streaming),
                Some(Marker::Remove { .. })
            ) {
                changed = true;
                let cluster_end = consume_removed_cluster(text, after_delims, streaming);
                let after_cluster = skip_span_delims(text, cluster_end);
                let next_ch = text[after_cluster..].chars().next();
                let drop_pre_space = match next_ch {
                    None => true,
                    Some(c) => {
                        c == '\n'
                            || c == '\r'
                            || is_horizontal_space(c)
                            || is_trailing_punctuation(c)
                    }
                };
                if !drop_pre_space {
                    out.push_str(&text[pos..space_end]);
                }
                pos = cluster_end;
                continue;
            }
            out.push_str(&text[pos..space_end]);
            pos = space_end;
            continue;
        }

        out.push(ch);
        if matches!(ch, '\n' | '\r') {
            at_line_start = true;
        } else if !is_horizontal_space(ch) {
            at_line_start = false;
        }
        pos += ch.len_utf8();
    }

    if changed {
        Cow::Owned(out)
    } else {
        Cow::Borrowed(text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fast_path_borrows_clean_text_unchanged() {
        let plain = "line1  \nline2 with `STIMER0` and 中文。";
        assert!(matches!(strip_citation_markers(plain), Cow::Borrowed(s) if s == plain));
    }

    #[test]
    fn strips_codex_cite_marker_and_orphaned_leading_space() {
        let raw = "保留 KVM 和现有磁盘。 \u{E200}cite\u{E202}turn137145search0\u{E201}";
        assert_eq!(strip_citation_markers(raw), "保留 KVM 和现有磁盘。");
    }

    #[test]
    fn collapses_double_space_around_inline_markers_and_clusters() {
        let mid = "Fixed in QEMU \u{E200}cite\u{E202}turn0search0\u{E201} and backported.";
        assert_eq!(strip_citation_markers(mid), "Fixed in QEMU and backported.");

        let multi = "Claim. \u{E200}cite\u{E202}turn0search0\u{E201} \u{E200}cite\u{E202}turn0search1\u{E201} Next.";
        assert_eq!(strip_citation_markers(multi), "Claim. Next.");

        let multi_eof = "Claim。\u{E200}cite\u{E202}turn0search0\u{E201} \u{E200}cite\u{E202}turn0search1\u{E201}";
        assert_eq!(strip_citation_markers(multi_eof), "Claim。");

        let before_punct =
            "Fixed in QEMU \u{E200}cite\u{E202}turn0search0\u{E201}, which resolved it.";
        assert_eq!(
            strip_citation_markers(before_punct),
            "Fixed in QEMU, which resolved it."
        );

        let before_word = "Fixed in QEMU \u{E200}cite\u{E202}turn0search0\u{E201}today.";
        assert_eq!(strip_citation_markers(before_word), "Fixed in QEMU today.");
    }

    #[test]
    fn preserves_entity_display_name_and_attributed_span_prose() {
        let entity = "See \u{E200}entity\u{E202}[\"software\",\"QEMU\",\"emulator\"]\u{E201} \u{E200}cite\u{E202}turn0view0\u{E201}.";
        assert_eq!(strip_citation_markers(entity), "See QEMU.");

        let span = "The \u{E203}Hyper-V STIMER0 deadlock\u{E204} \u{E200}cite\u{E202}turn0search0\u{E201} is known.";
        assert_eq!(
            strip_citation_markers(span),
            "The Hyper-V STIMER0 deadlock is known."
        );
    }

    #[test]
    fn withholds_incomplete_marker_while_streaming_and_cleans_truncated_tail() {
        assert_eq!(
            strip_citation_markers_streaming("保留磁盘。 \u{E200}"),
            "保留磁盘。"
        );
        assert_eq!(
            strip_citation_markers_streaming("保留磁盘。 \u{E200}ci"),
            "保留磁盘。"
        );
        assert_eq!(
            strip_citation_markers_streaming("保留磁盘。 \u{E200}cite\u{E202}turn137145"),
            "保留磁盘。"
        );
        assert_eq!(
            strip_citation_markers("保留磁盘。 \u{E200}cite\u{E202}turn137145"),
            "保留磁盘。"
        );
    }

    #[test]
    fn preserves_markdown_indentation_and_hard_line_breaks() {
        let md = "  - item \u{E200}cite\u{E202}turn0search0\u{E201}  \n  - next";
        assert_eq!(strip_citation_markers(md), "  - item  \n  - next");
    }
}
