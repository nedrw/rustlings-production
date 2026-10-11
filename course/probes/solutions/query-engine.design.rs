//! probe = "query/engine@design"
//! title = "A grammar, the refusals it types, and the plan it prints"
//! hint = "Read the text into tokens, refuse what this grammar does not have by category and at the byte it stood at, and print the plan as `key = value` lines with every key present."
//!
//! Step `lakelet/4.11-query-engine` is where a store stops answering one question and starts having
//! a *language*: a dashboard, an alert rule and another program write one string and read one exit
//! code, and the string is where the store decides what it will not do. Six decisions are the step,
//! and each of them is worth a sentence somebody had to write down:
//!
//! * **the time range is mandatory, and half-open.** `ts BETWEEN a AND b` means `[a, b)`: a record
//!   at `a` is in the answer and a record at `b` is the first record of the *next* question. Every
//!   range in this store is half-open already — `Range`, a bucket, a segment's stated span — and
//!   the *mandatory* half is the one that saves a fleet: a telemetry query that reads all of time
//!   is an accident, and it is refused in a sentence rather than answered with a scan;
//! * **a name is read, and a keyword is folded.** `FROM cpu` is exact, case included, because a
//!   filter that folded case would put one host's samples behind another's name — while `SELECT`,
//!   `FROM`, `WHERE`, `AND`, `GROUP BY` and the column `value` are compared without case, so a
//!   keyword is a *place in the grammar* rather than a reserved word, and a metric nobody would
//!   have chosen is still a name the store can hold;
//! * **`AND` only.** Every predicate has to hold and `OR` is refused rather than implemented: it is
//!   the one clause whose presence changes what a plan may prune, because a record that satisfies
//!   either half of a disjunction cannot be ruled out by a predicate that matches one half
//!   (`lakelet/4.14-index-pruning` argues it in full). It is refused *by category*, at the byte it
//!   stands on, and not by silence;
//! * **every refusal is typed, and carries its position.** A caller branches on the variant and a
//!   person reads the sentence: a `'…'` that never closes is refused at the quote that opened it,
//!   an aggregate nobody has carries the name exactly as it was written, an `OR` carries its own
//!   byte, and the class is the exit code `jobq/3.15-cli-contract` established, so a script reads
//!   a number and never a message;
//! * **the plan is a value a person reads and a script diffs.** `key = value` lines, one fact per
//!   line, and *every key is always present* — `window = none` rather than a missing line, because
//!   a reader comparing two plans must not have to notice that one of them is short. A plan says
//!   what a query *would* do before anything is opened, so it may not depend on what a file holds;
//! * **the answer's shape is decided by the text.** The window is the caller's to say and it sits
//!   on the absolute grid of `lakelet/4.10-downsampling`; whether an answer is one list or one per
//!   series is `GROUP BY …, series`; and everything the caller got wrong is decided **before a
//!   store is opened**, so a `2` is the caller's problem wherever the deployment happens to be.
//!
//! ## What you write
//!
//! * [`lex`] — the text as tokens: a word, a number, a `'…'` value and the four symbols
//!   `(` `)` `,` `=`, each remembering the byte it started at;
//! * [`parse`] — the grammar: the aggregate, the metric, the tag predicates, the mandatory
//!   half-open range, the optional window, and the refusals by category;
//! * [`render_plan`] — the plan as the `key = value` lines `EXPLAIN` prints.
//!
//! [`Kind`], [`Token`] and [`Token::spells`] are given, and so are [`Refused`] with [`Refused::at`]
//! and [`Refused::class`], [`Range`], [`Record`], [`Plan`] with the accessors that read it,
//! [`AGGREGATES`], [`COLUMNS`], [`unit_millis`] and [`symbol`]. The vocabulary *is* the interface
//! here: the four token kinds are the step's scanner's, the refusal variants are the categories a
//! caller branches on, `unit_millis` is the one copy of the table `--window 5m` and `time(5m)`
//! share, and `COLUMNS` is the three columns an answer is computed from — a file that let you
//! invent those would be checking a different step.
//!
//! ## What the tests ask
//!
//! The scanner's four kinds and the byte each token remembers; a `'…'` value that may hold `=`, `,`
//! and spaces, and one that never closes refused at the quote that opened it. An `OR` refused by
//! category at its own byte, and the same query with `AND` parsed instead. A column this grammar
//! does not aggregate refused at the column, in a sentence that names the one it has — while
//! `VALUE` is that column, because a keyword is compared without case. The range is mandatory (the
//! refusal names `ts BETWEEN`) and half-open, pinned with a record exactly on each edge: the record
//! at `from` is in the answer and the record at `to` is the next question's. Keywords are folded
//! and a name is kept exactly as it was written. The plan's keys are all present, `window = none`
//! where a query did not ask for one, and the same text twice prints the same lines. And a handful
//! of texts that are not this shape are refused as syntax, each at a byte inside the text.
//!
//! ## Where this is honest, and where it is not
//!
//! One file, no dependencies, **no store and no command line.** The step's real engine plans
//! *against a manifest*: its plan reads the segment spans to decide which files a query would
//! open, it refuses a range below the store's retention floor and a metric the store has never
//! held (class `3`, because a typo must not look like a quiet fleet), and `EXPLAIN` answers a store
//! whose segment files are *unreadable* — the strongest form of "it opened nothing". None of that
//! is here: what is here is the grammar and the refusals, which is the half of the step that a
//! probe can check in minutes and the half a design re-check is about.
//!
//! Two narrowings are stated rather than hidden. This file's grammar is a **subset**: it has no
//! `LIMIT` (a bound on how many series an answer covers), no `GROUP BY …, series` (the clause that
//! makes an answer per series) and no `EXPLAIN` statement form — a plan here is always printed,
//! because printing it is the exercise. And its window is **optional** where the step's is
//! mandatory, deliberately: a query that did not say how wide a bucket is still has a plan, and
//! that plan has to say `window = none`, which is how a reader sees that a key is never missing
//! rather than being told so. The keys themselves are the step's: `metric`, `aggregate`, `from`,
//! `to`, `window` and `columns` are printed by the real plan too, beside `series`,
//! `segments_opened`, `segments_pruned`, `records` and `limit`, which need a store to exist.

/// One token of a query, and the byte it started at.
///
/// The offset is what makes a refusal actionable: "an unterminated value" sends a reader back to
/// the whole query, and "an unterminated value at byte 41" sends them to the character.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Token {
    at: usize,
    kind: Kind,
}

impl Token {
    /// The byte this token starts at, counted from the start of the query.
    #[must_use]
    pub const fn at(&self) -> usize {
        self.at
    }

    /// What was read.
    #[must_use]
    pub const fn kind(&self) -> &Kind {
        &self.kind
    }

    /// `true` when this token is the word `keyword`, whatever case it was written in.
    ///
    /// This is the whole of "keywords are case-insensitive": there is no reserved word, only a
    /// place in the grammar that accepts one word and asks this question to find out.
    #[must_use]
    pub fn spells(&self, keyword: &str) -> bool {
        matches!(self.kind, Kind::Word(ref word) if word.eq_ignore_ascii_case(keyword))
    }
}

/// What one token is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Kind {
    /// A word: a keyword, a metric, a tag key, an aggregate name or a unit.
    Word(String),
    /// A run of digits, as the number it names. A run too large for a `u64` is refused rather than
    /// truncated: a millisecond count wraps, and a wrapped range is an answer about another time.
    Number(u64),
    /// A `'…'` value: the bytes between the quotes, with nothing changed and no escape rule.
    Text(String),
    /// One of the four symbols the grammar has: `(` `)` `,` `=`.
    Symbol(char),
}

/// The four symbols the grammar has, or `None` for a character that is part of a word.
#[must_use]
pub fn symbol(character: char) -> Option<char> {
    if "(),=".contains(character) {
        Some(character)
    } else {
        None
    }
}

/// Why a query was refused, and where in the text it was refused.
///
/// One variant per thing a caller can branch on: `Syntax` is the text itself — a `'…'` that never
/// closes, a clause that is missing, repeated or out of order, a column this grammar does not
/// aggregate — `Aggregate` is the name after `SELECT`, and `Disjunction` is the one clause whose
/// absence changes what a plan may prune. Every one of them carries a byte, because the byte is
/// what a reader acts on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refused {
    /// The text is not a query this grammar has.
    Syntax {
        /// Where the reader should look.
        at: usize,
        /// What is wrong with it, phrased for the person who typed the query.
        detail: String,
    },
    /// The name after `SELECT` is not one of the five aggregates. A near miss that quietly
    /// answered something else would be a plausible number for a question nobody asked.
    Aggregate {
        /// The name that was written.
        name: String,
        /// Where it was written.
        at: usize,
    },
    /// `OR`, which this grammar does not have.
    Disjunction {
        /// Where the `OR` was written.
        at: usize,
    },
}

impl Refused {
    /// Where in the text this refusal is about.
    #[must_use]
    pub const fn at(&self) -> usize {
        match self {
            Refused::Syntax { at, .. }
            | Refused::Aggregate { at, .. }
            | Refused::Disjunction { at } => *at,
        }
    }

    /// The class a script branches on: the exit code of the command line's table.
    ///
    /// `2` — the call is wrong — for every refusal in this file, because with no store behind the
    /// grammar there is no name the store has never held (`3`) and no store that could not answer
    /// (`4`). A caller reads the number and a person reads the sentence.
    #[must_use]
    pub const fn class(&self) -> u8 {
        2
    }
}

/// The sentence a person reads, next to the variant a script reads.
///
/// The two are one decision in the step's own material and they are kept apart here: a caller that
/// branched on this text would break the first time somebody reworded it, which is why the class
/// exists at all — so the sentence is allowed to say *where* and *what* for a reader who is about
/// to fix the query.
impl std::fmt::Display for Refused {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Refused::Syntax { at, detail } => write!(formatter, "at byte {at}, {detail}"),
            Refused::Aggregate { at, name } => write!(
                formatter,
                "at byte {at}, `{name}` is not an aggregate: they are {}",
                AGGREGATES.join(", ")
            ),
            Refused::Disjunction { at } => write!(
                formatter,
                "at byte {at}, `OR` is not part of this grammar: predicates are joined with \
                 `AND`, and every one of them has to hold"
            ),
        }
    }
}

/// The half-open span of time a query is about: `[from, to)`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Range {
    from_millis: u64,
    to_millis: u64,
}

impl Range {
    /// The range `[from_millis, to_millis)`.
    #[must_use]
    pub const fn new(from_millis: u64, to_millis: u64) -> Self {
        Self {
            from_millis,
            to_millis,
        }
    }

    /// The first millisecond the range is about.
    #[must_use]
    pub const fn from(&self) -> u64 {
        self.from_millis
    }

    /// The first millisecond the range is not about.
    #[must_use]
    pub const fn to(&self) -> u64 {
        self.to_millis
    }

    /// `true` when `ts_millis` is the range's: at or after its start and before its end.
    #[must_use]
    pub const fn contains(&self, ts_millis: u64) -> bool {
        self.from_millis <= ts_millis && ts_millis < self.to_millis
    }
}

/// One record of the store, so that the half-open rule can be pinned against a timestamp rather
/// than declared.
#[derive(Debug, Clone, PartialEq)]
pub struct Record {
    /// The series it belongs to: the metric and its tags, exactly as they were ingested.
    pub series: String,
    /// When it was measured, in milliseconds since the Unix epoch.
    pub ts_millis: i64,
    /// What it measured.
    pub value: f64,
}

/// The five aggregates this grammar has, in the order a refusal names them.
pub const AGGREGATES: [&str; 5] = ["count", "sum", "mean", "min", "max"];

/// The columns an answer is computed from, in the order a reader decodes them.
pub const COLUMNS: &str = "series,ts_millis,value";

/// How many milliseconds one window unit names, or `None` for a unit this grammar does not have.
///
/// The five units are `lakelet/4.10-downsampling`'s `--window` table, and this is the one copy of
/// it: `time(5m)` and `--window 5m` are the same window written two ways, and two copies of the
/// table would be two answers to what `90x` means.
#[must_use]
pub fn unit_millis(unit: &str) -> Option<u64> {
    match unit {
        "ms" => Some(1),
        "s" => Some(1_000),
        "m" => Some(60_000),
        "h" => Some(3_600_000),
        "d" => Some(86_400_000),
        _ => None,
    }
}

/// What one text means: the metric and the predicates it named, the aggregate, the range, and the
/// window when the query asked for one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plan {
    metric: String,
    predicates: Vec<(String, String)>,
    aggregate: String,
    range: Range,
    window: Option<u64>,
}

impl Plan {
    /// The metric `FROM` named, exactly as it was written.
    #[must_use]
    pub fn metric(&self) -> &str {
        &self.metric
    }

    /// The tag predicates, in the order they were written. Every one of them has to hold.
    #[must_use]
    pub fn predicates(&self) -> &[(String, String)] {
        &self.predicates
    }

    /// The aggregate, folded to one of [`AGGREGATES`].
    #[must_use]
    pub fn aggregate(&self) -> &str {
        &self.aggregate
    }

    /// The half-open range the query is about.
    #[must_use]
    pub const fn range(&self) -> Range {
        self.range
    }

    /// How wide one bucket is, or `None` when the query did not say.
    #[must_use]
    pub const fn window(&self) -> Option<u64> {
        self.window
    }
}

/// Read `text` as the tokens of a query.
///
/// The whole of the scanner is four cases, in this order: whitespace separates and disappears; a
/// `'` opens a value that runs to the next `'`; a digit opens a number that runs to the first
/// character that is not one; one of `(` `)` `,` `=` is a symbol; and anything else opens a word
/// that runs until a separator. Every token remembers the byte it began at.
///
/// # Errors
///
/// [`Refused::Syntax`] in exactly two cases, and it is the only refusal this stage produces: a `'`
/// that is never closed (refused at the quote that opened it), and a run of digits too large for
/// the `u64` this store states milliseconds in. Everything else is a token, including bytes nobody
/// would write on purpose — the scanner knows what a token is, the parser knows what a query is,
/// and neither guesses at the other's question.
pub fn lex(text: &str) -> Result<Vec<Token>, Refused> {
    let mut tokens = Vec::new();
    let mut characters = text.char_indices().peekable();
    while let Some((at, character)) = characters.next() {
        if character.is_whitespace() {
            continue;
        }
        if let Some(found) = symbol(character) {
            tokens.push(Token {
                at,
                kind: Kind::Symbol(found),
            });
            continue;
        }
        if character == '\'' {
            // A value runs to the next quote and may hold the characters the grammar would
            // otherwise read: `=`, a comma, and spaces are all bytes a client may have written.
            let mut value = String::new();
            let mut closed = false;
            for (_, next) in characters.by_ref() {
                if next == '\'' {
                    closed = true;
                    break;
                }
                value.push(next);
            }
            if !closed {
                return Err(Refused::Syntax {
                    at,
                    detail: "the value that opens here is never closed: write it as `'…'`, and \
                             a quote inside a value is a rule this grammar does not have"
                        .to_owned(),
                });
            }
            tokens.push(Token {
                at,
                kind: Kind::Text(value),
            });
            continue;
        }
        if character.is_ascii_digit() {
            let mut digits = String::from(character);
            while let Some(&(_, next)) = characters.peek() {
                if !next.is_ascii_digit() {
                    break;
                }
                digits.push(next);
                characters.next();
            }
            let Ok(number) = digits.parse::<u64>() else {
                return Err(Refused::Syntax {
                    at,
                    detail: format!(
                        "`{digits}` is larger than the millisecond count this store states, \
                         and a count that wrapped would be an answer about another time"
                    ),
                });
            };
            tokens.push(Token {
                at,
                kind: Kind::Number(number),
            });
            continue;
        }
        // Anything else opens a word, up to the next separator: a metric name is whatever bytes a
        // client ingested, and a word that means nothing to this grammar is refused by the parser
        // with the byte it stands at rather than by the scanner.
        let mut word = String::from(character);
        while let Some(&(_, next)) = characters.peek() {
            if next.is_whitespace() || next == '\'' || symbol(next).is_some() {
                break;
            }
            word.push(next);
            characters.next();
        }
        tokens.push(Token {
            at,
            kind: Kind::Word(word),
        });
    }
    Ok(tokens)
}

/// Read `text` as a query, or say what is wrong with it.
///
/// The grammar is the one this file documents: `SELECT <agg>(value) FROM <metric>` then, in this
/// order, an optional `WHERE` holding tag predicates and the range joined by `AND`, and an optional
/// `GROUP BY time(<n><unit>)`. The range is one of the predicates and it is mandatory — exactly
/// once — and `GROUP BY` may be left out, in which case the plan says so.
///
/// # Errors
///
/// [`Refused::Syntax`] when the text is not a query — a clause that is missing, repeated or out of
/// order, a column this grammar does not aggregate, a `'…'` that never closes, a run of digits
/// that is not a millisecond count — [`Refused::Aggregate`] when the name after `SELECT` is not one
/// of the five, and [`Refused::Disjunction`] for an `OR`.
pub fn parse(text: &str) -> Result<Plan, Refused> {
    // How a reader would spell a token back, for a sentence that says what was found instead.
    fn spelled(token: Option<&Token>) -> String {
        match token.map(Token::kind) {
            Some(Kind::Word(word)) => format!("`{word}`"),
            Some(Kind::Number(number)) => format!("`{number}`"),
            Some(Kind::Text(value)) => format!("'{value}'"),
            Some(Kind::Symbol(found)) => format!("`{found}`"),
            None => "the end of the query".to_owned(),
        }
    }

    // The byte a reader is looking at: the next token's, or the end of the text — a clause that is
    // *missing* is refused where a reader would have written it.
    fn at_of(tokens: &[Token], read: usize, end: usize) -> usize {
        tokens.get(read).map_or(end, Token::at)
    }

    // The refusal for a token that is not the one the grammar wants next.
    fn wanted(tokens: &[Token], read: usize, end: usize, what: &str) -> Refused {
        Refused::Syntax {
            at: at_of(tokens, read, end),
            detail: format!(
                "{} does not fit here: this grammar wants {what} next",
                spelled(tokens.get(read))
            ),
        }
    }

    // Read one word, one number, one value or one symbol, or refuse what is there instead.
    fn word(tokens: &[Token], read: &mut usize, end: usize, what: &str) -> Result<String, Refused> {
        match tokens.get(*read) {
            Some(Token {
                kind: Kind::Word(word),
                ..
            }) => {
                *read += 1;
                Ok(word.clone())
            }
            _ => Err(wanted(tokens, *read, end, what)),
        }
    }

    fn number(tokens: &[Token], read: &mut usize, end: usize, what: &str) -> Result<u64, Refused> {
        match tokens.get(*read) {
            Some(Token {
                kind: Kind::Number(number),
                ..
            }) => {
                *read += 1;
                Ok(*number)
            }
            _ => Err(wanted(tokens, *read, end, what)),
        }
    }

    fn value(
        tokens: &[Token],
        read: &mut usize,
        end: usize,
        what: &str,
    ) -> Result<String, Refused> {
        match tokens.get(*read) {
            Some(Token {
                kind: Kind::Text(value),
                ..
            }) => {
                *read += 1;
                Ok(value.clone())
            }
            _ => Err(wanted(tokens, *read, end, what)),
        }
    }

    fn punct(
        tokens: &[Token],
        read: &mut usize,
        end: usize,
        asked: char,
        what: &str,
    ) -> Result<(), Refused> {
        match tokens.get(*read) {
            Some(Token {
                kind: Kind::Symbol(found),
                ..
            }) if *found == asked => {
                *read += 1;
                Ok(())
            }
            _ => Err(wanted(tokens, *read, end, what)),
        }
    }

    // Read the word `keyword`, or refuse what is there: this is where the grammar compares without
    // case, and it is the only place a keyword means anything.
    fn keyword(
        tokens: &[Token],
        read: &mut usize,
        end: usize,
        keyword: &str,
        what: &str,
    ) -> Result<(), Refused> {
        match tokens.get(*read) {
            Some(token) if token.spells(keyword) => {
                *read += 1;
                Ok(())
            }
            _ => Err(wanted(tokens, *read, end, what)),
        }
    }

    fn spells(tokens: &[Token], read: usize, keyword: &str) -> bool {
        tokens.get(read).is_some_and(|token| token.spells(keyword))
    }

    let tokens = lex(text)?;
    let end = text.len();
    let mut read = 0usize;

    // `SELECT <agg>(value)`: the aggregate is folded, because that is where the grammar compares,
    // and `value` is the one column an aggregate here is over.
    keyword(&tokens, &mut read, end, "select", "the word `SELECT`")?;
    let at = at_of(&tokens, read, end);
    let name = word(&tokens, &mut read, end, "an aggregate name")?;
    let aggregate = name.to_ascii_lowercase();
    if !AGGREGATES.contains(&aggregate.as_str()) {
        return Err(Refused::Aggregate { name, at });
    }
    punct(&tokens, &mut read, end, '(', "an opening `(`")?;
    keyword(&tokens, &mut read, end, "value", "the column `value`")?;
    punct(&tokens, &mut read, end, ')', "a closing `)`")?;

    // `FROM <metric>`, kept exactly as it was written.
    keyword(&tokens, &mut read, end, "from", "the word `FROM`")?;
    let metric = word(&tokens, &mut read, end, "a metric name")?;

    // `WHERE <pred> [AND <pred>]*`: tag predicates and the range, in the caller's order, with the
    // range mandatory and written exactly once.
    let mut predicates: Vec<(String, String)> = Vec::new();
    let mut range: Option<Range> = None;
    if spells(&tokens, read, "where") {
        read += 1;
        loop {
            let at = at_of(&tokens, read, end);
            if spells(&tokens, read, "ts") {
                read += 1;
                if range.is_some() {
                    return Err(Refused::Syntax {
                        at,
                        detail: "the time range was given twice: a query is about one span of \
                                 time, written once as `ts BETWEEN a AND b`"
                            .to_owned(),
                    });
                }
                if !spells(&tokens, read, "between") {
                    return Err(Refused::Syntax {
                        at,
                        detail: "`ts` is the time axis and not a tag: the range is written `ts \
                                 BETWEEN a AND b`, and a tag predicate is `<key> = '<value>'`"
                            .to_owned(),
                    });
                }
                read += 1;
                let from = number(
                    &tokens,
                    &mut read,
                    end,
                    "the first millisecond of the range",
                )?;
                keyword(&tokens, &mut read, end, "and", "the word `AND`")?;
                let to = number(
                    &tokens,
                    &mut read,
                    end,
                    "the millisecond the range stops before",
                )?;
                if to <= from {
                    return Err(Refused::Syntax {
                        at,
                        detail: format!(
                            "`ts BETWEEN {from} AND {to}` is the empty range [{from}, {to}): \
                             the range is half-open, so its first number is smaller than its \
                             second"
                        ),
                    });
                }
                range = Some(Range::new(from, to));
            } else {
                // An `OR` stands where a predicate would, and it is refused there rather than
                // read as one: it is the clause that would change what a plan may prune.
                if spells(&tokens, read, "or") {
                    return Err(Refused::Disjunction { at });
                }
                let key = word(&tokens, &mut read, end, "a tag key")?;
                punct(&tokens, &mut read, end, '=', "an `=`")?;
                let value = value(
                    &tokens,
                    &mut read,
                    end,
                    "a value in single quotes, as in `host = 'h0'`",
                )?;
                predicates.push((key, value));
            }
            if spells(&tokens, read, "and") {
                read += 1;
                continue;
            }
            // `OR` stands where the next `AND` would, which is where a reader writes it.
            if spells(&tokens, read, "or") {
                return Err(Refused::Disjunction {
                    at: at_of(&tokens, read, end),
                });
            }
            break;
        }
    }

    // `GROUP BY time(<n><unit>)`, when the query said how wide a bucket is.
    let mut window: Option<u64> = None;
    if spells(&tokens, read, "group") {
        read += 1;
        keyword(&tokens, &mut read, end, "by", "the word `BY`")?;
        keyword(&tokens, &mut read, end, "time", "the word `time`")?;
        punct(&tokens, &mut read, end, '(', "an opening `(`")?;
        let count = number(&tokens, &mut read, end, "a number of milliseconds")?;
        let at = at_of(&tokens, read, end);
        let unit = word(&tokens, &mut read, end, "a unit")?;
        let Some(factor) = unit_millis(&unit) else {
            return Err(Refused::Syntax {
                at,
                detail: format!(
                    "`{unit}` is not a unit this grammar has: they are `ms`, `s`, `m`, `h` and \
                     `d`"
                ),
            });
        };
        let Some(width) = count.checked_mul(factor) else {
            return Err(Refused::Syntax {
                at,
                detail: format!(
                    "`{count}{unit}` is wider than the millisecond count this store states"
                ),
            });
        };
        if width == 0 {
            return Err(Refused::Syntax {
                at,
                detail: "`time(0ms)` has no bucket in it: a window is a positive number of \
                         milliseconds, and a range that counts in nothing has no answer"
                    .to_owned(),
            });
        }
        window = Some(width);
        punct(&tokens, &mut read, end, ')', "a closing `)`")?;
    }

    // Nothing may follow: a clause written twice, or a word that fits nowhere, is refused where it
    // stands rather than ignored — a query that quietly answers something else is worse than one
    // that says no.
    if let Some(token) = tokens.get(read) {
        return Err(Refused::Syntax {
            at: token.at(),
            detail: format!(
                "{} does not fit here: a query is `SELECT <agg>(value) FROM <metric> [WHERE \
                 <pred> [AND <pred>]*] [GROUP BY time(<n><unit>)]`, and each clause is written \
                 once, in that order",
                spelled(Some(token))
            ),
        });
    }

    let Some(range) = range else {
        return Err(Refused::Syntax {
            at: end,
            detail: "there is no `ts BETWEEN a AND b`: a query has to say which span of time it \
                     is about, because a question that reads all of time is not a question — it \
                     is an accident with a fleet behind it"
                .to_owned(),
        });
    };

    Ok(Plan {
        metric,
        predicates,
        aggregate,
        range,
        window,
    })
}

/// The plan as the command line's `EXPLAIN` prints it: `key = value` lines a human reads.
///
/// Every key is always present, and `none` is how a plan says there is nothing to say —
/// `window = none` rather than a missing line, and `predicates = none` for a query that named no
/// tag — because a plan is a value a script diffs and a ticket quotes: a reader comparing two
/// plans must not have to notice that one of them is short.
#[must_use]
pub fn render_plan(plan: &Plan) -> String {
    let window = match plan.window() {
        Some(width) => width.to_string(),
        None => "none".to_owned(),
    };
    let predicates = if plan.predicates().is_empty() {
        "none".to_owned()
    } else {
        plan.predicates()
            .iter()
            .map(|(key, value)| format!("{key}={value}"))
            .collect::<Vec<String>>()
            .join(",")
    };
    [
        ("metric", plan.metric().to_owned()),
        ("predicates", predicates),
        ("aggregate", plan.aggregate().to_owned()),
        ("from", plan.range().from().to_string()),
        ("to", plan.range().to().to_string()),
        ("window", window),
        ("columns", COLUMNS.to_owned()),
    ]
    .into_iter()
    .map(|(key, value)| format!("{key} = {value}\n"))
    .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One record of `series` at the absolute instant `ts_millis`.
    fn stamped(series: &str, ts_millis: i64, value: f64) -> Record {
        Record {
            series: series.to_owned(),
            ts_millis,
            value,
        }
    }

    /// The bytes a text holds, for a case that pins the position a refusal names.
    fn byte_of(text: &str, needle: &str) -> usize {
        text.find(needle)
            .unwrap_or_else(|| panic!("`{text}` holds no `{needle}`"))
    }

    /// The scanner reads four kinds of token and remembers the byte each one started at.
    #[test]
    fn the_scanner_reads_words_numbers_values_and_symbols() {
        let one = lex("select").unwrap_or_else(|refusal| panic!("one word is lexable: {refusal}"));
        assert_eq!(one.len(), 1, "one word is one token");
        assert_eq!(one[0].at(), 0, "a token remembers the byte it started at");
        assert_eq!(one[0].kind(), &Kind::Word("select".to_owned()));
        assert!(
            one[0].spells("SELECT"),
            "and a keyword is compared without case"
        );
        assert!(one[0].spells("Select"));
        assert!(!one[0].spells("selec"));

        // The four symbols, the numbers of a range, and a value that holds the characters the
        // grammar would otherwise read.
        let text = "MEAN(value) host = 'a=b,c d' 1700000000000";
        let tokens = lex(text).unwrap_or_else(|refusal| panic!("`{text}` is lexable: {refusal}"));
        let kinds: Vec<&Kind> = tokens.iter().map(Token::kind).collect();
        assert_eq!(
            kinds,
            [
                &Kind::Word("MEAN".to_owned()),
                &Kind::Symbol('('),
                &Kind::Word("value".to_owned()),
                &Kind::Symbol(')'),
                &Kind::Word("host".to_owned()),
                &Kind::Symbol('='),
                &Kind::Text("a=b,c d".to_owned()),
                &Kind::Number(1_700_000_000_000),
            ],
            "in the order they were written, with the quotes taken off the value"
        );
        assert_eq!(tokens[1].at(), 4, "`(` is the fifth byte");
        assert_eq!(
            tokens[6].at(),
            byte_of(text, "'"),
            "and the value starts at the quote a reader can count to"
        );

        // Whitespace separates and is otherwise nothing; a name may hold the characters a metric
        // name is usually made of, because the store holds whatever clients wrote.
        let spaced =
            lex("a  b\n\tcpu.load-1").unwrap_or_else(|refusal| panic!("lexable: {refusal}"));
        let words: Vec<&str> = spaced
            .iter()
            .map(|token| match token.kind() {
                Kind::Word(word) => word.as_str(),
                other => panic!("words, and this is {other:?}"),
            })
            .collect();
        assert_eq!(words, ["a", "b", "cpu.load-1"]);

        // A window is a number and the unit right after it, and a run of digits this store cannot
        // state is refused rather than wrapped.
        let window = lex("5m").unwrap_or_else(|refusal| panic!("lexable: {refusal}"));
        assert_eq!(window[0].kind(), &Kind::Number(5));
        assert_eq!(window[1].kind(), &Kind::Word("m".to_owned()));
        let Err(too_large) = lex("99999999999999999999999") else {
            panic!("a number larger than a u64 is not a millisecond count this store states")
        };
        assert!(matches!(too_large, Refused::Syntax { .. }));
        assert_eq!(too_large.class(), 2);
    }

    /// A value that never closes is refused at the quote that opened it rather than at the end of
    /// the text a reader would have to search.
    #[test]
    fn an_unterminated_value_is_refused_at_the_quote_that_opened_it() {
        let text = "SELECT count(value) FROM cpu WHERE host = 'h0 GROUP BY time(1m)";
        let Err(refusal) = lex(text) else {
            panic!("a value that never closes is not a query this scanner reads")
        };
        let quote = byte_of(text, "'");
        match &refusal {
            Refused::Syntax { at, detail } => {
                assert_eq!(
                    *at, quote,
                    "the refusal names the quote that opened the value — byte {quote} — and not \
                     the end of the text"
                );
                assert!(
                    !detail.is_empty(),
                    "and a refusal carries the sentence a reader acts on"
                );
            }
            other => panic!("an unterminated value is a syntax refusal, and this is {other:?}"),
        }
        assert_eq!(refusal.at(), quote, "the byte is readable off the refusal");
        assert_eq!(refusal.class(), 2, "and it is the caller's class");

        // The other direction: the same text with the value closed is a query, so the refusal is
        // about the quote and not about values in general.
        assert!(
            lex("SELECT count(value) FROM cpu WHERE host = 'h0' GROUP BY time(1m)").is_ok(),
            "a value that closes is a token like any other"
        );
    }

    /// The time predicate is mandatory and half-open, pinned with a record exactly on each edge.
    #[test]
    fn the_time_predicate_is_mandatory_and_half_open() {
        let missing = parse("SELECT count(value) FROM cpu GROUP BY time(1m)")
            .expect_err("a query has to say which span of time it is about");
        match &missing {
            Refused::Syntax { detail, .. } => assert!(
                detail.contains("ts BETWEEN"),
                "the refusal names the clause the query has to have: {detail}"
            ),
            other => {
                panic!("a query with no time predicate is a syntax refusal, and this is {other:?}")
            }
        }

        // Half-open, asserted from both sides rather than declared: a record whose timestamp is
        // exactly `from` is in the answer and one whose timestamp is exactly `to` is the first
        // record of the next question.
        let text = "SELECT count(value) FROM cpu WHERE host = 'h0' \
                    AND ts BETWEEN 1700000000000 AND 1700000060000 GROUP BY time(1m)";
        let plan = parse(text).unwrap_or_else(|refusal| panic!("a query of this shape: {refusal}"));
        assert_eq!(plan.range().from(), 1_700_000_000_000);
        assert_eq!(plan.range().to(), 1_700_000_060_000);
        let records = [
            stamped("cpu,host=h0", 1_699_999_999_999, 1.0),
            stamped("cpu,host=h0", 1_700_000_000_000, 2.0),
            stamped("cpu,host=h0", 1_700_000_059_999, 3.0),
            stamped("cpu,host=h0", 1_700_000_060_000, 4.0),
        ];
        let answered: Vec<i64> = records
            .iter()
            .filter(|record| {
                plan.range()
                    .contains(u64::try_from(record.ts_millis).unwrap_or(0))
            })
            .map(|record| record.ts_millis)
            .collect();
        assert_eq!(
            answered,
            vec![1_700_000_000_000, 1_700_000_059_999],
            "the record at `from` is in the answer and the record at `to` belongs to the next \
             question"
        );

        // The range is one predicate among the others, so its order in the clause is the caller's.
        assert!(
            parse("SELECT count(value) FROM cpu WHERE ts BETWEEN 0 AND 60 AND host = 'h0'").is_ok(),
            "the time predicate is a predicate like the others, and `AND` joins them in any order"
        );
    }

    /// `OR` is refused by category, at its own byte, and never by silence.
    #[test]
    fn an_or_is_refused_by_category_and_not_by_silence() {
        let text = "SELECT count(value) FROM cpu WHERE host = 'a' OR host = 'b' \
                    AND ts BETWEEN 0 AND 60";
        let refusal = parse(text).expect_err("`OR` is not part of this grammar");
        match &refusal {
            Refused::Disjunction { at } => assert_eq!(
                *at,
                byte_of(text, "OR"),
                "the refusal names the byte the `OR` stands at"
            ),
            other => panic!("an `OR` is a disjunction refusal, and this is {other:?}"),
        }
        assert_eq!(refusal.class(), 2, "and it is the caller's class");

        // Both directions: `AND` is the grammar's, so the same query joined with `AND` parses, and
        // an `OR` standing where the next `AND` would is the same refusal rather than a surprise.
        assert!(
            parse(
                "SELECT count(value) FROM cpu WHERE host = 'a' AND host = 'b' \
                   AND ts BETWEEN 0 AND 60"
            )
            .is_ok(),
            "every predicate has to hold, and `AND` is how a caller says so"
        );
        let trailing =
            parse("SELECT count(value) FROM cpu WHERE ts BETWEEN 0 AND 60 OR host = 'a'")
                .expect_err("an `OR` after the time predicate is the same refusal");
        assert!(
            matches!(trailing, Refused::Disjunction { .. }),
            "and it is still a disjunction refusal: {trailing:?}"
        );
    }

    /// A column this grammar does not aggregate is refused where it stands, in a sentence that
    /// names the one column it has.
    #[test]
    fn an_unknown_column_is_refused_with_its_position() {
        let text = "SELECT count(ts) FROM cpu WHERE ts BETWEEN 0 AND 60";
        let refusal = parse(text).expect_err("`value` is the one column an aggregate is over");
        match &refusal {
            Refused::Syntax { at, detail } => {
                assert_eq!(
                    *at,
                    byte_of(text, "ts)"),
                    "the refusal points at the column, not at the query"
                );
                assert!(
                    detail.contains("value"),
                    "and the sentence names the column this grammar aggregates: {detail}"
                );
            }
            other => {
                panic!("a column nobody aggregates is a syntax refusal, and this is {other:?}")
            }
        }

        // The column the grammar has parses, whatever case it was written in: `value` is a keyword
        // and a keyword is compared without case, which is what makes `count(ts)` a mistake about
        // the column rather than a second column nobody declared.
        assert!(parse("SELECT count(value) FROM cpu WHERE ts BETWEEN 0 AND 60").is_ok());
        assert!(
            parse("SELECT count(VALUE) FROM cpu WHERE ts BETWEEN 0 AND 60").is_ok(),
            "a keyword is folded where the grammar compares it"
        );
    }

    /// The plan prints `key = value` lines, every key is present, and the same text twice prints
    /// the same lines.
    #[test]
    fn the_plan_prints_the_keys_a_human_reads() {
        let text = "SELECT mean(value) FROM cpu WHERE host = 'h0' \
                    AND ts BETWEEN 1700000000000 AND 1700000060000 GROUP BY time(5m)";
        let plan = parse(text).unwrap_or_else(|refusal| panic!("a query of this shape: {refusal}"));
        let lines = render_plan(&plan);
        for key in [
            "metric",
            "predicates",
            "aggregate",
            "from",
            "to",
            "window",
            "columns",
        ] {
            assert!(
                lines
                    .lines()
                    .any(|line| line.starts_with(&format!("{key} = "))),
                "every key is always present, and `{key}` is missing from:\n{lines}"
            );
        }
        assert!(
            lines.contains("window = 300000"),
            "a five-minute window is 300000 milliseconds: {lines}"
        );
        assert!(lines.contains("aggregate = mean"), "{lines}");
        assert!(
            lines.contains("metric = cpu"),
            "the metric is what `FROM` named: {lines}"
        );
        assert!(
            lines.ends_with('\n'),
            "and the last line ends, or `while read` drops it"
        );
        assert_eq!(
            lines,
            render_plan(
                &parse(text).unwrap_or_else(|refusal| panic!("the same text again: {refusal}"))
            ),
            "the same input twice prints the same lines: a plan is a value a reviewer diffs"
        );

        // A query that did not say how wide a bucket is says so rather than hiding the line.
        let no_window = parse("SELECT count(value) FROM cpu WHERE ts BETWEEN 0 AND 60")
            .unwrap_or_else(|refusal| panic!("the window is not mandatory here: {refusal}"));
        assert_eq!(no_window.window(), None);
        assert!(
            render_plan(&no_window).contains("window = none"),
            "a key is never absent, and `none` is how a plan says there is nothing to say:\n{}",
            render_plan(&no_window)
        );
        assert!(
            render_plan(&no_window).contains("predicates = none"),
            "the same for a query that named no tag"
        );
    }

    /// Keywords are folded and names are kept exactly as they were written.
    #[test]
    fn the_grammar_folds_keywords_and_keeps_names() {
        let text = "select MEAN(Value) from CPU where Host = 'H0' \
                    and TS between 0 and 60 group by TIME(1m)";
        let plan = parse(text)
            .unwrap_or_else(|refusal| panic!("keywords are compared without case: {refusal}"));
        assert_eq!(
            plan.metric(),
            "CPU",
            "a name is kept exactly as it was written: a grammar that folded case here would put \
             one host's samples behind another's name"
        );
        assert_eq!(
            plan.predicates(),
            &[("Host".to_owned(), "H0".to_owned())],
            "and a tag key and its value too"
        );
        assert_eq!(
            plan.aggregate(),
            "mean",
            "while the aggregate is folded to one of the five, because that is where the grammar \
             compares"
        );
        assert_eq!(plan.window(), Some(60_000), "and `TIME(1m)` is a minute");

        // A name that is nearly an aggregate is a typo, and a typo that answered would be a
        // plausible number for a question nobody asked.
        let refusal = parse("SELECT average(value) FROM cpu WHERE ts BETWEEN 0 AND 60")
            .expect_err("`average` is not one of the five");
        match &refusal {
            Refused::Aggregate { name, .. } => assert_eq!(
                name, "average",
                "the refusal carries the name as it was written, not as it was folded"
            ),
            other => {
                panic!("an aggregate nobody has is an aggregate refusal, and this is {other:?}")
            }
        }
    }

    /// A text that is not this shape is refused as syntax, at a byte inside the text.
    #[test]
    fn a_query_that_is_not_the_shape_is_refused_where_it_stops() {
        for (text, what) in [
            (
                "UPDATE cpu SET value = 1",
                "a statement that is not this one",
            ),
            ("SELECT count(value) FROM cpu", "no time predicate at all"),
            (
                "SELECT count(value) FROM cpu WHERE ts BETWEEN 0 AND 60 GROUP BY time(90x)",
                "a unit nobody has",
            ),
            (
                "SELECT count(value) FROM cpu WHERE ts BETWEEN 0 AND 60 FROM cpu",
                "a clause after the last one",
            ),
            (
                "SELECT count(value) FROM cpu WHERE ts BETWEEN 5 AND 5",
                "a range that ends where it starts",
            ),
            (
                "SELECT count(value) FROM cpu WHERE ts BETWEEN 0 AND 60 GROUP BY time(1m), \
                 series",
                "a clause this subset does not have: a `,` fits nowhere in it",
            ),
        ] {
            let refusal = parse(text).unwrap_err();
            assert!(
                matches!(refusal, Refused::Syntax { .. }),
                "`{text}` ({what}) is a syntax refusal, and this is {refusal:?}"
            );
            assert!(
                refusal.at() <= text.len(),
                "`{text}` ({what}) is refused at a byte inside the text, and {} is not one",
                refusal.at()
            );
        }

        // And the shape that is this grammar's parses: the cases above are refusals rather than a
        // scanner that cannot read anything.
        assert!(
            parse("SELECT count(value) FROM cpu WHERE ts BETWEEN 0 AND 60").is_ok(),
            "a query of this grammar's shape"
        );
    }
}
