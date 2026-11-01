// External scanner for Bynk docs: doc blocks and one-line docs.
//
// A doc block is a multi-line construct of the form:
//
//   ---
//   content lines...
//   ---
//
// The opening marker is a line consisting of three or more consecutive
// hyphens (followed only by horizontal whitespace and a newline / EOF).
// The closing marker is another such line with exactly as many hyphens as
// the opener (#1885), so a `----` block can hold `---` lines. Content
// between the markers is arbitrary text and may include `--` line-comment
// fragments without terminating the block.
//
// A one-line doc (#2203) is a marker of three or more hyphens, at least one
// space or tab, then text, first on its line. It is scanned here rather than
// by a regex token because only the scanner can tell a marker line (a block
// fence) from a marker with text, and can see that nothing but indentation
// precedes it on its line. The token runs to the end of the line, newline
// excluded.
//
// The scanner must run before the regex tokenizer because tree-sitter's
// regex flavour disallows lazy quantifiers and look-around, so the
// arbitrary content between markers cannot be expressed inline.

#include "tree_sitter/parser.h"

#include <wctype.h>

enum TokenType {
    DOC_BLOCK,
    DOC_LINE,
    RAW_STRING,
};

void *tree_sitter_bynk_external_scanner_create(void) { return NULL; }
void tree_sitter_bynk_external_scanner_destroy(void *p) { (void)p; }
unsigned tree_sitter_bynk_external_scanner_serialize(void *p, char *buf) {
    (void)p; (void)buf; return 0;
}
void tree_sitter_bynk_external_scanner_deserialize(void *p, const char *b, unsigned n) {
    (void)p; (void)b; (void)n;
}

// Advance the lexer one byte.
static inline void advance(TSLexer *l) { l->advance(l, false); }
// Skip-advance (does not contribute to the token span).
static inline void skip(TSLexer *l) { l->advance(l, true); }

// Try to match a hyphen-only "marker line": three or more `-`, then only
// horizontal whitespace, then a newline (or EOF). On success, consume the
// line (through the newline) and return true. Leaves the lexer positioned
// at the next byte. The lexer must be at the first non-whitespace byte of
// the line on entry — leading horizontal whitespace must already be
// skipped by the caller.
static bool match_marker_line(TSLexer *l) {
    int dashes = 0;
    while (l->lookahead == '-') {
        advance(l);
        dashes++;
    }
    if (dashes < 3) {
        return false;
    }
    // Trailing horizontal whitespace only.
    while (l->lookahead == ' ' || l->lookahead == '\t' || l->lookahead == '\r') {
        advance(l);
    }
    if (l->lookahead == '\n') {
        advance(l);
        return true;
    }
    if (l->lookahead == 0) {
        // EOF after marker; accept.
        return true;
    }
    return false;
}

// #2619: a raw string literal — `r"…"`, `r#"…"#` (any number of `#`), or the
// multi-line `r"""…"""`. No escapes, no interpolation, so the body is opaque:
// only the closing delimiter matters, and counting `#`s is beyond a regex
// token. The lexer is at the `r` on entry.
static bool scan_raw_string(TSLexer *l) {
    advance(l); // `r`
    int hashes = 0;
    while (l->lookahead == '#') {
        advance(l);
        hashes++;
    }
    if (l->lookahead != '"') {
        return false;
    }
    advance(l); // opening `"`
    if (hashes == 0 && l->lookahead == '"') {
        advance(l); // second quote: `r""` (empty) or the start of `r"""`
        if (l->lookahead != '"') {
            l->result_symbol = RAW_STRING;
            return true;
        }
        advance(l); // `r"""`: a multi-line raw string closes at the next `"""`
        int quotes = 0;
        while (l->lookahead != 0) {
            if (l->lookahead == '"') {
                quotes++;
                advance(l);
                if (quotes == 3) {
                    l->result_symbol = RAW_STRING;
                    return true;
                }
            } else {
                quotes = 0;
                advance(l);
            }
        }
        return false;
    }
    // Single line: the body runs to a `"` followed by `hashes` `#`s.
    while (l->lookahead != 0 && l->lookahead != '\n') {
        if (l->lookahead == '"') {
            advance(l);
            int seen = 0;
            while (seen < hashes && l->lookahead == '#') {
                advance(l);
                seen++;
            }
            if (seen == hashes) {
                l->result_symbol = RAW_STRING;
                return true;
            }
        } else {
            advance(l);
        }
    }
    return false;
}

bool tree_sitter_bynk_external_scanner_scan(void *payload, TSLexer *lexer,
                                            const bool *valid_symbols) {
    (void)payload;
    if (!valid_symbols[DOC_BLOCK] && !valid_symbols[DOC_LINE] && !valid_symbols[RAW_STRING]) {
        return false;
    }
    // A one-line doc must be first on its line: the scanner starts at column
    // 0, or skips a newline on its way to the marker.
    bool first_on_line = lexer->get_column(lexer) == 0;
    // The lexer's position when external scanners run can include leading
    // whitespace — and, crucially, the scanner is frequently invoked at the
    // newline *before* the marker line (after the preceding item). We must
    // skip whitespace *including newlines* to reach the opening marker;
    // otherwise the internal lexer reads the `---` as operators first.
    // Skipped bytes are only committed if we ultimately return a token, so
    // over-skipping past a non-marker is harmless (we bail).
    while (lexer->lookahead == ' ' || lexer->lookahead == '\t' ||
           lexer->lookahead == '\r' || lexer->lookahead == '\n') {
        if (lexer->lookahead == '\n') {
            first_on_line = true;
        }
        skip(lexer);
    }
    if (lexer->lookahead == 'r') {
        return valid_symbols[RAW_STRING] && scan_raw_string(lexer);
    }
    if (lexer->lookahead != '-') {
        return false;
    }
    // Need to see at least three dashes to be a marker.
    // We can't peek-ahead arbitrarily; advance one at a time, counting.
    int dashes = 0;
    while (lexer->lookahead == '-') {
        advance(lexer);
        dashes++;
    }
    if (dashes < 3) {
        return false;
    }
    bool spaced = lexer->lookahead == ' ' || lexer->lookahead == '\t';
    // Trailing horizontal whitespace on the opening marker line.
    while (lexer->lookahead == ' ' || lexer->lookahead == '\t' || lexer->lookahead == '\r') {
        advance(lexer);
    }
    if (lexer->lookahead != '\n') {
        // #2203: text after a spaced marker, first on its line, is a one-line
        // doc. Anything else (`---x`, a marker after code) is not a doc.
        if (spaced && first_on_line && lexer->lookahead != 0 && valid_symbols[DOC_LINE]) {
            while (lexer->lookahead != 0 && lexer->lookahead != '\n') {
                advance(lexer);
            }
            lexer->result_symbol = DOC_LINE;
            return true;
        }
        return false;
    }
    if (!valid_symbols[DOC_BLOCK]) {
        return false;
    }
    advance(lexer); // consume opening newline

    // Now scan content lines until we hit another marker line.
    while (lexer->lookahead != 0) {
        // Skip leading horizontal whitespace on this line.
        while (lexer->lookahead == ' ' || lexer->lookahead == '\t' || lexer->lookahead == '\r') {
            advance(lexer);
        }
        // Try to match a closing marker.
        if (lexer->lookahead == '-') {
            int local_dashes = 0;
            while (lexer->lookahead == '-') {
                advance(lexer);
                local_dashes++;
            }
            // #1885: only a marker of the opener's length closes the block,
            // like a Markdown code fence; any other marker line is content.
            if (local_dashes == dashes) {
                // After dashes, only horizontal whitespace then newline / EOF.
                while (lexer->lookahead == ' ' || lexer->lookahead == '\t' ||
                       lexer->lookahead == '\r') {
                    advance(lexer);
                }
                if (lexer->lookahead == '\n') {
                    advance(lexer);
                    lexer->result_symbol = DOC_BLOCK;
                    return true;
                }
                if (lexer->lookahead == 0) {
                    lexer->result_symbol = DOC_BLOCK;
                    return true;
                }
                // Dashes but not a clean marker; continue as content.
            }
            // Continue consuming the rest of the line as content.
        }
        // Consume to end of line.
        while (lexer->lookahead != 0 && lexer->lookahead != '\n') {
            advance(lexer);
        }
        if (lexer->lookahead == '\n') {
            advance(lexer);
        }
    }
    // EOF without closing marker — accept as a (malformed) doc block so
    // the editor still highlights what's there.
    lexer->result_symbol = DOC_BLOCK;
    return true;
}
