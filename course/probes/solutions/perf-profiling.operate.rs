//! probe = "perf/profiling@operate"
//! title = "Reading a sampler's table, and naming the phase that owns it"
//! hint = "Read the table into one count per symbol, sort it by self time with the name breaking a tie, and give the phase mapping the whole run in permille — reporting the symbols no phase claims instead of dropping them."
//!
//! Step `lakelet/4.16-profiling` is the step where a counter is not enough. `lakelet/4.15` counts the
//! work somebody remembered to instrument, and the number that was never counted is the number that
//! decides the incident: the store's own page found the process waiting for the device, in a symbol
//! no phase charges, while every phase counter was green. The instrument that finds that is a
//! **sampler** — it interrupts whatever is running instead of reporting on what somebody instrumented
//! — and this file is the other half of the step: reading what the sampler wrote.
//!
//! Four decisions, and each one is the difference between evidence and an anecdote:
//!
//! * **the profile is a table, and a reader that skips a line it cannot read is a reader that reports
//!   a share of a whole it does not have.** One line per leaf frame, `count symbol`, and a line the
//!   reader refuses is refused with the line in it rather than dropped;
//! * **self time is a count, so two runs of one file print the same lines.** Heaviest first, and a tie
//!   broken by the symbol's own name: a table whose order depended on how the file happened to be
//!   sorted would not be a table two people can compare;
//! * **the shares are permille of the whole run, and they add up.** Whole permille of a whole run
//!   leaves a remainder, and the buckets with the largest remainders take it — the same rule
//!   `lakelet/4.15-budget-regression`'s `Trace::shares` uses for the phases, and the reason a table
//!   of shares never prints 998;
//! * **a symbol in no phase is reported, and it is usually the finding.** Writing the phase map is
//!   reading the call graph above the leaf table, and what the map does not cover is where the process
//!   actually was: on this project's own run, nine hundred and sixty of the run's thousand permille
//!   were symbols no phase charges.
//!
//! ## What you write
//!
//! * [`read`] — a sampler's table into one count per symbol;
//! * [`top_by_self`] — the heaviest symbols, in the order a report prints them;
//! * [`attribution`] — which phase owns the most self time, what share, and what nothing owns.
//!
//! [`Sample`], [`Profile`], [`PhaseMap`], [`Attribution`] and [`Refused`] are given: the phase names
//! are `lakelet/4.15-budget-regression`'s seven, and a probe that let you invent them would be
//! checking a different reading of a profile.
//!
//! ## What the tests ask
//!
//! The table: one count per symbol, a symbol the sampler printed twice accumulates, an empty table is
//! an empty profile, and a line whose first field is not a count, a line with a count and no symbol
//! and a line with a third field are three refusals that carry the line they refused. The top of a
//! profile: heaviest first, a tie between two symbols broken by name, nothing asked for is nothing
//! answered, and more asked for than there is answers with all of it. The attribution: the shares of
//! the whole run in permille and the unowned permille add up to a thousand; the symbols no phase
//! claims are reported heaviest first rather than dropped; a mapping that names every symbol reports
//! nothing unowned and its shares are the whole run; an equal share is broken by name; a mapping that
//! names one symbol in two phases is still a function; and a profile with no samples is not a
//! division.
//!
//! ## Where this is honest, and where it is not
//!
//! One file, no dependencies, **no sampler and no process**: the real step builds a release
//! `lakelet-load`, runs `--profile --seconds 30`, samples its pid with `sample <pid> <seconds> -file
//! out.txt` — the profiler `kiln doctor` reports as the one that works on this platform without
//! root — and pastes the report's last section into `docs/profile.md`. The numbers on that page are
//! one machine's, at one build, on one afternoon, which is exactly why the page carries the run that
//! produced them. What is re-checked here is reading a profile: the shape of the table, the order a
//! report prints it in, the shares, and naming the phase that moved.
//!
//! Also missing on purpose: the sampler's own report is not a table. `/usr/bin/sample` prints a call
//! graph first and then a leaf-frame table with three columns — the symbol, the image it came from,
//! and the count — and the symbols are mangled and long
//! (`_RNvMNtNtNtCs1eoBWF22PJM_7lakelet6engine5index5bloomNtB2_5Bloom6insert`). This file's format is
//! that table with the columns the other way round and the image column dropped, and it uses the
//! short names a reader writes down beside the mangled ones: what is being read is a profile's
//! *shape*. And nothing here is a flame graph: a profile with a call graph in it is a tree, and
//! attributing a tree is a different question — the sampler's own report is the only thing that says
//! where a leaf came from.

use std::collections::BTreeMap;

/// One row of a sampler's table: a symbol, and how many samples the process was in it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sample {
    /// The symbol, exactly as the table spelled it.
    pub symbol: String,
    /// How many samples landed in it.
    pub count: u64,
}

/// A profile: what a sampler saw, one count per symbol.
///
/// Given. The counts are *self* time — how many times the sampler interrupted the process inside
/// that symbol — rather than a subtree's total, because that is what the leaf table reports and what
/// decides where the time went.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Profile {
    /// One count per symbol, in a sorted map so two readings of one table are the same value.
    counts: BTreeMap<String, u64>,
}

impl Profile {
    /// A profile with nothing in it.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Count `count` more samples in `symbol`.
    pub fn add(&mut self, symbol: &str, count: u64) {
        *self.counts.entry(symbol.to_owned()).or_insert(0) += count;
    }

    /// How many samples landed in one symbol, and `0` for one the sampler never landed in.
    #[must_use]
    pub fn count(&self, symbol: &str) -> u64 {
        self.counts.get(symbol).copied().unwrap_or(0)
    }

    /// How many samples the whole profile is: the denominator every share is a share of.
    #[must_use]
    pub fn total(&self) -> u64 {
        self.counts.values().copied().sum()
    }

    /// `true` when the sampler landed in nothing.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.counts.is_empty()
    }

    /// Every symbol and its count, in name order. A report sorts this; it is not in weight order,
    /// because a map that reordered itself would make two readings of one table two values.
    pub fn iter(&self) -> impl Iterator<Item = (&str, u64)> {
        self.counts
            .iter()
            .map(|(symbol, count)| (symbol.as_str(), *count))
    }
}

/// Which phase a symbol belongs to: the mapping a reader writes down once, from the call graph above
/// the leaf table and from what each symbol does in this store.
///
/// Given, because the mapping is the reader's own work and this file is about what comes out of it —
/// and because a mapping with a shape of its own would be a second thing to re-check.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PhaseMap {
    /// One entry per phase, in the order the phases run: the phase's name, and the symbols it owns.
    entries: Vec<(String, Vec<String>)>,
}

impl PhaseMap {
    /// A map from one row per phase: its name, and the symbols that phase owns.
    #[must_use]
    pub fn of(entries: &[(&str, &[&str])]) -> Self {
        Self {
            entries: entries
                .iter()
                .map(|(name, owned)| {
                    (
                        (*name).to_owned(),
                        owned.iter().map(|symbol| (*symbol).to_owned()).collect(),
                    )
                })
                .collect(),
        }
    }

    /// The phase that owns `symbol`, or `None` for a symbol in no phase.
    ///
    /// The first phase that names it wins, so a mapping that lists a symbol twice is still a function
    /// rather than a question.
    #[must_use]
    pub fn phase_of(&self, symbol: &str) -> Option<&str> {
        self.entries
            .iter()
            .find(|(_, owned)| owned.iter().any(|owned| owned == symbol))
            .map(|(name, _)| name.as_str())
    }

    /// Every phase and the symbols it owns, in the order the phases run.
    pub fn entries(&self) -> impl Iterator<Item = (&str, &[String])> {
        self.entries
            .iter()
            .map(|(name, owned)| (name.as_str(), owned.as_slice()))
    }
}

/// What a profile's self time belongs to.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Attribution {
    /// One entry per phase that owns at least one sample: its name, and its share of the whole
    /// profile in permille, heaviest first, ties by name.
    pub shares: Vec<(String, u64)>,
    /// The symbols no phase claims, heaviest first, ties by name. A mapping is only as good as the
    /// call graph it was written from, and the symbols it misses are where the process really was.
    pub unowned: Vec<Sample>,
    /// Permille of the whole profile the unowned symbols account for.
    pub unowned_permille: u64,
}

impl Attribution {
    /// The phase that owns the most self time, and its share in permille.
    #[must_use]
    pub fn top(&self) -> Option<(&str, u64)> {
        self.shares
            .first()
            .map(|(phase, permille)| (phase.as_str(), *permille))
    }

    /// `true` when the mapping covered every symbol the sampler landed in.
    #[must_use]
    pub fn covered(&self) -> bool {
        self.unowned.is_empty()
    }
}

/// Why a table could not be read.
///
/// One class per mistake, and every one of them carries the line: a profile that skipped a line it
/// could not read would answer with shares of a whole it does not have.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refused {
    /// The first field is not a decimal count.
    NotACount {
        /// The line, as it was read.
        line: String,
    },
    /// A count with nothing after it: a count of no symbol is not a row of a table.
    NoSymbol {
        /// The line, as it was read.
        line: String,
    },
    /// A third field. A symbol with a space in it would be two symbols and a count belonging to
    /// neither of them.
    ExtraField {
        /// The line, as it was read.
        line: String,
    },
}

/// Read a sampler's table into a profile.
///
/// One line per leaf frame: `<count> <symbol>`, which is `/usr/bin/sample`'s last section with the
/// columns the other way round. Blank lines and lines whose first character is `#` are skipped, and
/// the counts of a symbol that appears twice are added together — a table merged out of two samples
/// is still a table.
///
/// # Errors
///
/// [`Refused::NotACount`], [`Refused::NoSymbol`] or [`Refused::ExtraField`], naming the first line
/// that is not a row: the count is read first, then the symbol, then the question of a third field.
pub fn read(table: &str) -> Result<Profile, Refused> {
    let mut profile = Profile::new();
    for line in table.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let mut fields = trimmed.split_whitespace();
        let Some(count) = fields.next().and_then(|field| field.parse::<u64>().ok()) else {
            return Err(Refused::NotACount {
                line: trimmed.to_owned(),
            });
        };
        match (fields.next(), fields.next()) {
            (Some(symbol), None) => profile.add(symbol, count),
            (None, _) => {
                return Err(Refused::NoSymbol {
                    line: trimmed.to_owned(),
                });
            }
            (Some(_), Some(_)) => {
                return Err(Refused::ExtraField {
                    line: trimmed.to_owned(),
                });
            }
        }
    }
    Ok(profile)
}

/// The heaviest symbols of a profile, in the order a report prints them.
///
/// Heaviest first, and a tie broken by the symbol's own name: two runs over one table have to print
/// the same lines, or the table is not something two people can compare. Asking for nothing answers
/// with nothing rather than with everything, which is the one off-by-one this function has.
#[must_use]
pub fn top_by_self(profile: &Profile, wanted: usize) -> Vec<Sample> {
    use std::cmp::Reverse;

    let mut samples: Vec<Sample> = profile
        .iter()
        .map(|(symbol, count)| Sample {
            symbol: symbol.to_owned(),
            count,
        })
        .collect();
    samples.sort_by(|left, right| {
        Reverse(left.count)
            .cmp(&Reverse(right.count))
            .then_with(|| left.symbol.cmp(&right.symbol))
    });
    samples.truncate(wanted);
    samples
}

/// Attribute a profile's self time to the phases of `phases`, and say what nothing owns.
///
/// The shares are permille of the **whole** profile — the symbols no phase claims included — so the
/// shares and [`Attribution::unowned_permille`] are together a thousand for any profile with samples
/// in it. Whole permille of a whole run leaves a remainder, and the buckets with the largest
/// remainders take it, with the bucket's name breaking a tie: 998 permille is a profile somebody has
/// to explain. A profile with no samples is not a division at all, and answers with nothing.
#[must_use]
pub fn attribution(profile: &Profile, phases: &PhaseMap) -> Attribution {
    use std::cmp::Reverse;

    let total = profile.total();
    if total == 0 {
        return Attribution::default();
    }
    let names: Vec<&str> = phases.entries().map(|(name, _)| name).collect();

    // Where each symbol's samples go: into the phase that owns it, or into the bucket nobody owns.
    let mut owned = vec![0_u64; names.len()];
    let mut unowned: Vec<Sample> = Vec::new();
    for (symbol, count) in profile.iter() {
        match phases
            .phase_of(symbol)
            .and_then(|name| names.iter().position(|known| *known == name))
        {
            Some(index) => owned[index] += count,
            None => unowned.push(Sample {
                symbol: symbol.to_owned(),
                count,
            }),
        }
    }
    let unowned_total: u64 = unowned.iter().map(|sample| sample.count).sum();

    // One bucket per phase that saw a sample, and one the mapping did not cover. A bucket is an
    // index rather than a name, because the uncovered one has no name of its own.
    let mut buckets: Vec<(Option<usize>, u64)> = owned
        .iter()
        .enumerate()
        .filter(|(_, count)| **count > 0)
        .map(|(index, count)| (Some(index), *count))
        .collect();
    if unowned_total > 0 {
        buckets.push((None, unowned_total));
    }

    let whole = i128::from(total);
    let mut shares: Vec<(Option<usize>, u64, i128)> = buckets
        .into_iter()
        .map(|(bucket, count)| {
            let scaled = i128::from(count) * 1000;
            (bucket, (scaled / whole) as u64, scaled % whole)
        })
        .collect();
    let given: u64 = shares.iter().map(|(_, permille, _)| *permille).sum();
    // The bucket's name is what breaks a tie between two equal remainders, and the bucket nobody owns
    // has no name of its own: it sorts under the word the report uses for it rather than dropping out
    // of the tie-break.
    let name_of = |bucket: Option<usize>| bucket.map_or("unowned", |index| names[index]);
    shares.sort_by(|left, right| {
        Reverse(left.2)
            .cmp(&Reverse(right.2))
            .then_with(|| name_of(left.0).cmp(name_of(right.0)))
    });
    let leftover = usize::try_from(1000_u64.saturating_sub(given)).unwrap_or(0);
    for (_, permille, _) in shares.iter_mut().take(leftover) {
        *permille += 1;
    }

    let mut attribution = Attribution::default();
    for (bucket, permille, _) in shares {
        match bucket {
            Some(index) => attribution.shares.push((names[index].to_owned(), permille)),
            None => attribution.unowned_permille = permille,
        }
    }
    attribution.shares.sort_by(|left, right| {
        Reverse(left.1)
            .cmp(&Reverse(right.1))
            .then_with(|| left.0.cmp(&right.0))
    });
    unowned.sort_by(|left, right| {
        Reverse(left.count)
            .cmp(&Reverse(right.count))
            .then_with(|| left.symbol.cmp(&right.symbol))
    });
    attribution.unowned = unowned;
    attribution
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The leaf-frame table of this project's own run, as `docs/profile.md` quotes it: the process
    /// waiting for the device, and the read half of the loop small beside it.
    const TABLE: &str = "\
# Sort by top of stack, same collapsed (when >= 5):

2180 __fcntl
393 __open
160 __rename
119 write
53 crc32_hasher_update
45 bloom_insert
20 zstd_decompress_sequences
";

    /// The phases those symbols belong to: the two the leaf table can be attributed to, and nothing
    /// for the syscalls — which is this project's finding rather than a gap in the table.
    fn mapping() -> PhaseMap {
        PhaseMap::of(&[
            ("headers", &["bloom_insert"]),
            ("decode", &["crc32_hasher_update"]),
            ("decompress", &["zstd_decompress_sequences"]),
        ])
    }

    /// A table is one count per symbol, and a symbol printed twice is one symbol.
    #[test]
    fn a_table_reads_into_one_count_per_symbol() {
        let profile = read(TABLE).expect("this table is the format the reader reads");
        assert_eq!(
            profile.total(),
            2970,
            "the counts add up to the whole run: 2180 + 393 + 160 + 119 + 53 + 45 + 20"
        );
        assert_eq!(profile.count("__fcntl"), 2180);
        assert_eq!(
            profile.count("nobody_ever_called_this"),
            0,
            "a symbol the sampler never landed in is a zero, not an error"
        );
        assert_eq!(profile.iter().count(), 7, "one row per symbol");

        let twice = read("2180 __fcntl\n2180 __fcntl\n").expect("two rows, one symbol");
        assert_eq!(
            twice.count("__fcntl"),
            4360,
            "a table merged out of two samples is still a table: the counts of one symbol add up"
        );
        assert!(
            read("").expect("an empty table").is_empty(),
            "and a run with no rows is an empty profile rather than a refusal"
        );
    }

    /// A reader that skips a line it cannot read reports a share of a whole it does not have.
    #[test]
    fn a_line_the_reader_cannot_read_is_refused_rather_than_dropped() {
        assert_eq!(
            read("2180 __fcntl\nfive __open\n"),
            Err(Refused::NotACount {
                line: "five __open".to_owned()
            }),
            "a table with a line the reader cannot read is not a table it can answer from"
        );
        assert_eq!(
            read("393\n"),
            Err(Refused::NoSymbol {
                line: "393".to_owned()
            }),
            "a count of nothing is not a row"
        );
        assert_eq!(
            read("393 __open extra\n"),
            Err(Refused::ExtraField {
                line: "393 __open extra".to_owned()
            }),
            "a third field is a symbol with a space in it, which is two symbols and one count"
        );
        assert_eq!(
            read("  \n\n# a comment\n20 zstd_decompress_sequences\n")
                .map(|profile| profile.total()),
            Ok(20),
            "blank lines and comments are the sampler's own headings, and skipping them is not \
             skipping evidence"
        );
    }

    /// The top of a profile is a deterministic order, and a tie is broken by name.
    #[test]
    fn the_top_of_a_profile_is_heaviest_first_and_a_tie_breaks_by_name() {
        let profile = read(TABLE).expect("this table is the format the reader reads");
        let heaviest = top_by_self(&profile, 3);
        let top: Vec<&str> = heaviest
            .iter()
            .map(|sample| sample.symbol.as_str())
            .collect();
        assert_eq!(
            top,
            vec!["__fcntl", "__open", "__rename"],
            "the three heaviest symbols, in the order a report prints them"
        );
        assert_eq!(top_by_self(&profile, 1)[0].count, 2180);

        let tied = read("900 beta\n900 alpha\n10 gamma\n").expect("three rows");
        assert_eq!(
            top_by_self(&tied, 2)
                .iter()
                .map(|sample| sample.symbol.as_str())
                .collect::<Vec<&str>>(),
            vec!["alpha", "beta"],
            "two symbols with the same count come out in name order, so two runs over one file \
             print the same lines"
        );
        assert_eq!(
            top_by_self(&profile, 0),
            Vec::new(),
            "asking for nothing answers with nothing rather than with everything"
        );
        assert_eq!(
            top_by_self(&profile, 99).len(),
            7,
            "and asking for more than there is answers with all of it"
        );
    }

    /// The shares are of the whole run, and the symbols no phase claims are most of this one.
    #[test]
    fn the_shares_are_the_whole_run_and_what_no_phase_owns_is_most_of_it() {
        let profile = read(TABLE).expect("this table is the format the reader reads");
        let attribution = attribution(&profile, &mapping());

        assert_eq!(
            attribution.top(),
            Some(("decode", 18)),
            "the biggest phase of the three the leaf table can be attributed to is 53 samples in \
             seventeen permille, rounded up by the remainder rule to eighteen"
        );
        assert_eq!(
            attribution.shares,
            vec![
                ("decode".to_owned(), 18),
                ("headers".to_owned(), 15),
                ("decompress".to_owned(), 7),
            ],
            "heaviest first, and the two per mille the floors left went to the largest remainders \
             rather than to nobody"
        );
        assert_eq!(
            attribution.unowned_permille, 960,
            "nine hundred and sixty of the run's thousand permille are symbols no phase charges: \
             the page's own finding is that a counter and a sampler are two instruments"
        );
        assert_eq!(
            attribution
                .shares
                .iter()
                .map(|(_, permille)| *permille)
                .sum::<u64>()
                + attribution.unowned_permille,
            1000,
            "and the shares and the unowned permille are together the whole run, which is what makes \
             them shares rather than seven tallies"
        );
    }

    /// A symbol in no phase is reported with its name and its count, never dropped.
    #[test]
    fn a_symbol_in_no_phase_is_reported_heaviest_first() {
        let profile = read(TABLE).expect("this table is the format the reader reads");
        let attribution = attribution(&profile, &mapping());
        assert!(
            !attribution.covered(),
            "a mapping that leaves the device wait out is not a mapping that covered the run"
        );
        assert_eq!(
            attribution
                .unowned
                .iter()
                .map(|sample| sample.symbol.as_str())
                .collect::<Vec<&str>>(),
            vec!["__fcntl", "__open", "__rename", "write"],
            "every symbol no phase claims, heaviest first, ties by name"
        );
        assert_eq!(
            attribution
                .unowned
                .iter()
                .map(|sample| sample.count)
                .sum::<u64>(),
            2852,
            "and their counts are the ones the table gave them: reporting a symbol and inventing a \
             count for it would be two mistakes"
        );
    }

    /// A mapping that names every symbol reports nothing unowned, and its shares are the whole run.
    #[test]
    fn a_mapping_that_covers_every_symbol_leaves_nothing_unowned() {
        let profile = read(TABLE).expect("this table is the format the reader reads");
        let everything = PhaseMap::of(&[
            ("headers", &["__fcntl", "__open"]),
            (
                "decode",
                &[
                    "__rename",
                    "write",
                    "crc32_hasher_update",
                    "bloom_insert",
                    "zstd_decompress_sequences",
                ],
            ),
        ]);
        let attribution = attribution(&profile, &everything);
        assert!(
            attribution.covered(),
            "every symbol this run landed in belongs to a phase: {:?}",
            attribution.unowned
        );
        assert_eq!(attribution.unowned_permille, 0);
        assert_eq!(
            attribution.shares,
            vec![("headers".to_owned(), 866), ("decode".to_owned(), 134)],
            "and the shares are still the whole run in permille: 2573 samples of 2970 are 866, and \
             the phase that owns the rest carries the remainder"
        );
        assert_eq!(
            attribution
                .shares
                .iter()
                .map(|(_, permille)| *permille)
                .sum::<u64>(),
            1000,
            "with nothing left over, because nothing is unowned"
        );
    }

    /// An equal share is broken by name, so two readings of one table print one table.
    #[test]
    fn two_phases_that_own_the_same_amount_come_out_in_name_order() {
        let profile = read("100 alpha\n100 beta\n300 gamma\n").expect("three rows");
        let map = PhaseMap::of(&[("render", &["alpha"]), ("blocks", &["beta"])]);
        let attribution = attribution(&profile, &map);
        assert_eq!(
            attribution.shares,
            vec![("blocks".to_owned(), 200), ("render".to_owned(), 200)],
            "two phases at two hundred permille each, in name order"
        );
        assert_eq!(attribution.top(), Some(("blocks", 200)));
        assert_eq!(
            attribution.unowned_permille, 600,
            "and the symbol neither phase owns is six hundred permille of the run"
        );
    }

    /// A mapping that names one symbol twice is still a function, and an empty profile is not a
    /// division.
    #[test]
    fn a_mapping_is_a_function_and_an_empty_profile_is_not_a_division() {
        let profile = read("100 shared\n").expect("one row");
        let twice = PhaseMap::of(&[("filter", &["shared"]), ("aggregate", &["shared"])]);
        assert_eq!(
            attribution(&profile, &twice).shares,
            vec![("filter".to_owned(), 1000)],
            "the first phase that names a symbol owns it, so a mapping that lists it twice answers \
             one way rather than two ways"
        );

        let empty = read("# nothing was sampled\n").expect("an empty table");
        assert!(empty.is_empty());
        assert!(top_by_self(&empty, 3).is_empty());
        let attribution = attribution(&empty, &mapping());
        assert!(
            attribution.shares.is_empty() && attribution.unowned.is_empty(),
            "a run with no samples has no shares and nothing unowned: {attribution:?}"
        );
        assert_eq!(
            attribution.unowned_permille, 0,
            "and no permille at all, because there is no whole to divide"
        );
        assert_eq!(
            attribution.top(),
            None,
            "and no phase owns the most of nothing"
        );
    }
}
