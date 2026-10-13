//! Term dictionary: puts product names and jargon into their canonical spelling in recognised text.
//!
//! The recogniser writes "гугл мит", "джира", "Google Mit" or "ios" where the speaker said Google
//! Meet, Jira or iOS. Every word window of one to three tokens is compared to the known terms on a
//! phonetic *skeleton* (Cyrillic transliterated, English digraphs and vowel spellings folded) with a
//! normalised edit distance. The match is deliberately conservative:
//!
//! * whole tokens only — a term is never matched inside a longer word;
//! * skeletons shorter than 5 letters must be equal, longer ones may differ by at most 20 %
//!   (at most one edit below 10 letters) and must start alike;
//! * "jargon" (`release`, `deploy`, ...) is only repaired when written in Latin letters, because the
//!   Russian spelling of an everyday loanword is already correct;
//! * ambiguous English words (`notion`, `slack`, `windows`) are only capitalised inside text that
//!   contains Cyrillic, and a small stoplist protects ordinary Russian words that happen to sound
//!   like a term (`телеграмма`).

use yc_core::transcript::{Segment, TextCorrection};

/// How a term is matched.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    /// Product name: fuzzy match in any script.
    Brand,
    /// Everyday technical word written in lower case: fuzzy match on Latin text only, and a
    /// capital first letter is kept.
    Jargon,
    /// Acronym or mixed-case name: case-insensitive Latin equality plus listed spellings only.
    Exact,
}

#[derive(Debug, Clone)]
struct Term {
    canonical: String,
    skeleton: String,
    kind: Kind,
    /// Only corrected inside text that contains Cyrillic when matched from Latin letters.
    ambiguous: bool,
    /// Listed alternative spellings (lower case tokens), matched exactly.
    aliases: Vec<Vec<String>>,
}

/// Canonical terms plus the user's own; see the module docs.
#[derive(Debug, Clone)]
pub struct TermDictionary {
    terms: Vec<Term>,
}

/// Ordinary Russian words that sound close to a term and must never be touched.
const STOPLIST: &[&str] = &[
    "телеграмма",
    "телеграммы",
    "телеграмме",
    "телеграмму",
    "слабо",
    "слабый",
    "миро",
];

/// `(canonical, kind, ambiguous, aliases)`; aliases are space-separated tokens.
type Builtin = (&'static str, Kind, bool, &'static [&'static str]);

const BUILTIN: &[Builtin] = &[
    (
        "Google Meet",
        Kind::Brand,
        false,
        &["гугл мит", "гугл миты"],
    ),
    ("Zoom", Kind::Brand, false, &[]),
    ("Telegram", Kind::Brand, false, &[]),
    ("Slack", Kind::Brand, true, &["слэк"]),
    ("Jira", Kind::Brand, false, &["жира", "джире", "джиру"]),
    ("Confluence", Kind::Brand, false, &["конфлюэнс"]),
    ("GitHub", Kind::Brand, false, &["гитхаб", "гит хаб"]),
    ("GitLab", Kind::Brand, false, &["гитлаб", "гит лаб"]),
    ("Figma", Kind::Brand, false, &["фигма"]),
    ("Notion", Kind::Brand, true, &["ноушен", "ноушн", "ношн"]),
    ("Miro", Kind::Brand, true, &[]),
    ("Android", Kind::Brand, false, &[]),
    ("Windows", Kind::Brand, true, &["виндоус"]),
    ("Linux", Kind::Brand, false, &[]),
    ("Skype", Kind::Brand, false, &[]),
    ("Discord", Kind::Brand, false, &[]),
    ("Docker", Kind::Brand, false, &[]),
    (
        "Kubernetes",
        Kind::Brand,
        false,
        &["кубернетис", "кубернетес"],
    ),
    ("Trello", Kind::Brand, false, &["трелло"]),
    ("Excel", Kind::Brand, false, &["эксель"]),
    (
        "WhatsApp",
        Kind::Brand,
        false,
        &["вотсап", "ватсап", "вацап"],
    ),
    ("YouTube", Kind::Brand, false, &["ютуб", "ю туб"]),
    ("Python", Kind::Brand, false, &["пайтон"]),
    ("iOS", Kind::Exact, false, &["айос", "ай ос"]),
    ("macOS", Kind::Exact, false, &["макос", "мак ос"]),
    ("API", Kind::Exact, false, &["апи", "эй пи ай"]),
    ("SDK", Kind::Exact, false, &["эс ди кей"]),
    ("MVP", Kind::Exact, false, &["эм ви пи", "мвп"]),
    ("OKR", Kind::Exact, false, &["оу кей ар"]),
    ("KPI", Kind::Exact, false, &["кипиай", "кей пи ай", "кпи"]),
    ("UI", Kind::Exact, false, &["юай", "ю ай"]),
    ("UX", Kind::Exact, false, &["юикс", "ю икс"]),
    (
        "A/B-тест",
        Kind::Exact,
        false,
        &["а б тест", "аб тест", "эй би тест", "a b тест", "a b test"],
    ),
    ("backend", Kind::Jargon, false, &[]),
    ("frontend", Kind::Jargon, false, &[]),
    ("release", Kind::Jargon, false, &[]),
    ("deploy", Kind::Jargon, false, &[]),
];

impl TermDictionary {
    /// The built-in list only.
    pub fn builtin() -> Self {
        Self::with_user_terms(std::iter::empty::<&str>())
    }

    /// The built-in list plus the user's terms (a user term replaces a built-in with the same
    /// spelling). The kind of a user term follows its shape: all lower case → jargon, short all
    /// capitals → acronym, otherwise a product name.
    pub fn with_user_terms<I, S>(user: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let mut terms: Vec<Term> = BUILTIN
            .iter()
            .map(|(canonical, kind, ambiguous, aliases)| Term {
                canonical: (*canonical).to_owned(),
                skeleton: skeleton_of(&tokens(canonical)),
                kind: *kind,
                ambiguous: *ambiguous,
                aliases: aliases.iter().map(|a| alias_tokens(a)).collect(),
            })
            .collect();
        for raw in user {
            let canonical = raw.as_ref().trim();
            if canonical.chars().count() < 2 || canonical.chars().count() > 40 {
                continue;
            }
            let has_lower = canonical.chars().any(char::is_lowercase);
            let has_upper = canonical.chars().any(char::is_uppercase);
            let kind = if !has_upper {
                Kind::Jargon
            } else if !has_lower && canonical.chars().count() <= 6 {
                Kind::Exact
            } else {
                Kind::Brand
            };
            let term = Term {
                canonical: canonical.to_owned(),
                skeleton: skeleton_of(&tokens(canonical)),
                kind,
                ambiguous: false,
                aliases: Vec::new(),
            };
            terms.retain(|t| !t.canonical.eq_ignore_ascii_case(canonical));
            terms.push(term);
        }
        Self { terms }
    }

    /// Corrects `text`; returns the new text and the replacements made (empty when unchanged).
    pub fn correct(&self, text: &str) -> (String, Vec<TextCorrection>) {
        let toks = scan(text);
        let cyrillic_text = text.chars().any(is_cyrillic);
        let mut out = String::with_capacity(text.len());
        let mut done = 0usize; // byte offset up to which `text` is already in `out`
        let mut corrections = Vec::new();
        let mut i = 0usize;
        while i < toks.len() {
            match self.best_match(text, &toks, i, cyrillic_text) {
                Some((len, replacement)) => {
                    let (start, end) = (toks[i].start, toks[i + len - 1].end);
                    let original = &text[start..end];
                    if original != replacement {
                        out.push_str(&text[done..start]);
                        out.push_str(&replacement);
                        done = end;
                        corrections.push(TextCorrection {
                            original: original.to_owned(),
                            replacement,
                        });
                    }
                    i += len;
                }
                None => i += 1,
            }
        }
        out.push_str(&text[done..]);
        (out, corrections)
    }

    /// Corrects a segment's text in place and records the replacements in `corrections`.
    pub fn correct_segment(&self, segment: &mut Segment) {
        let (text, corrections) = self.correct(&segment.text);
        if !corrections.is_empty() {
            segment.text = text;
            segment.corrections.extend(corrections);
        }
    }

    /// The best term for a window starting at token `i`: `(tokens consumed, replacement)`.
    fn best_match(
        &self,
        text: &str,
        toks: &[Tok],
        i: usize,
        cyrillic_text: bool,
    ) -> Option<(usize, String)> {
        let mut best: Option<(f32, usize, &Term)> = None; // (distance, window, term)
        for len in 1..=3usize.min(toks.len() - i) {
            let window = &toks[i..i + len];
            if len > 1
                && !window
                    .windows(2)
                    .all(|w| joinable(&text[w[0].end..w[1].start]))
            {
                break;
            }
            let words: Vec<String> = window.iter().map(|t| t.lower.clone()).collect();
            if words.iter().any(|w| STOPLIST.contains(&w.as_str())) {
                continue;
            }
            let latin = !words.iter().any(|w| w.chars().any(is_cyrillic));
            for term in &self.terms {
                let Some(distance) = score(term, &words, latin, cyrillic_text) else {
                    continue;
                };
                let better =
                    best.is_none_or(|(d, l, _)| distance < d || (distance == d && len > l));
                if better {
                    best = Some((distance, len, term));
                }
            }
        }
        let (_, len, term) = best?;
        let original = &text[toks[i].start..toks[i + len - 1].end];
        let capital = original.chars().next().is_some_and(char::is_uppercase);
        let mut replacement = term.canonical.clone();
        if term.kind == Kind::Jargon && capital {
            let mut chars = replacement.chars();
            if let Some(first) = chars.next() {
                replacement = first.to_uppercase().chain(chars).collect();
            }
        }
        // A jargon word that only differs by its capital is already right.
        if term.kind == Kind::Jargon && original.eq_ignore_ascii_case(&term.canonical) {
            return Some((len, original.to_owned()));
        }
        Some((len, replacement))
    }
}

/// Match quality of a window against a term: `Some(normalised distance)`, `None` = no match.
fn score(term: &Term, words: &[String], latin: bool, cyrillic_text: bool) -> Option<f32> {
    if term.aliases.iter().any(|a| a == words) {
        return Some(0.0);
    }
    if latin && term.ambiguous && !cyrillic_text {
        return None;
    }
    match term.kind {
        Kind::Exact => {
            let canonical = tokens(&term.canonical);
            (latin && canonical.len() == words.len() && *canonical == *words).then_some(0.0)
        }
        Kind::Jargon if !latin => None,
        Kind::Brand | Kind::Jargon => {
            let source = tokens(&term.canonical).len();
            if words.len() + 1 < source || words.len() > source + 1 {
                return None;
            }
            let window = skeleton_of(words);
            fuzzy(&window, &term.skeleton)
        }
    }
}

/// Normalised edit distance of two skeletons under the conservative acceptance rule.
fn fuzzy(window: &str, term: &str) -> Option<f32> {
    let (a, b): (Vec<char>, Vec<char>) = (window.chars().collect(), term.chars().collect());
    let longest = a.len().max(b.len());
    if a.is_empty() || b.is_empty() || a[0] != b[0] {
        return None;
    }
    let distance = levenshtein(&a, &b);
    let allowed = match b.len() {
        0..=4 => 0,
        5..=9 => 1,
        _ => 2,
    };
    (distance <= allowed).then_some(distance as f32 / longest as f32)
}

fn levenshtein(a: &[char], b: &[char]) -> usize {
    let mut row: Vec<usize> = (0..=b.len()).collect();
    for (i, &ca) in a.iter().enumerate() {
        let mut prev = row[0];
        row[0] = i + 1;
        for (j, &cb) in b.iter().enumerate() {
            let cur = row[j + 1];
            row[j + 1] = (prev + usize::from(ca != cb)).min(row[j] + 1).min(cur + 1);
            prev = cur;
        }
    }
    row[b.len()]
}

#[derive(Debug)]
struct Tok {
    start: usize,
    end: usize,
    lower: String,
}

/// Whole-word tokens (runs of letters and digits) with byte spans.
fn scan(text: &str) -> Vec<Tok> {
    let mut out = Vec::new();
    let mut start: Option<usize> = None;
    for (at, c) in text.char_indices() {
        match (c.is_alphanumeric(), start) {
            (true, None) => start = Some(at),
            (false, Some(s)) => {
                out.push(tok(text, s, at));
                start = None;
            }
            _ => {}
        }
    }
    if let Some(s) = start {
        out.push(tok(text, s, text.len()));
    }
    out
}

fn tok(text: &str, start: usize, end: usize) -> Tok {
    Tok {
        start,
        end,
        lower: normalise_word(&text[start..end]),
    }
}

fn normalise_word(word: &str) -> String {
    word.chars()
        .flat_map(char::to_lowercase)
        .map(|c| if c == 'ё' { 'е' } else { c })
        .collect()
}

/// Word tokens of a canonical spelling or alias (lower case).
fn tokens(spelling: &str) -> Vec<String> {
    scan(spelling).into_iter().map(|t| t.lower).collect()
}

fn alias_tokens(alias: &str) -> Vec<String> {
    tokens(alias)
}

/// Separators that may sit inside one multi-word term.
fn joinable(separator: &str) -> bool {
    !separator.is_empty()
        && separator
            .chars()
            .all(|c| matches!(c, ' ' | '\u{a0}' | '-' | '/'))
}

fn is_cyrillic(c: char) -> bool {
    matches!(c, '\u{0400}'..='\u{04FF}')
}

/// Phonetic skeleton of a token sequence (see the module docs).
fn skeleton_of(words: &[String]) -> String {
    words.iter().map(|w| word_skeleton(w)).collect()
}

fn word_skeleton(word: &str) -> String {
    let mut latin = String::with_capacity(word.len() + 2);
    for c in word.chars() {
        match c {
            'а' => latin.push('a'),
            'б' => latin.push('b'),
            'в' => latin.push('v'),
            'г' => latin.push('g'),
            'д' => latin.push('d'),
            'е' | 'э' => latin.push('e'),
            'ё' => latin.push_str("io"),
            'ж' => latin.push('J'),
            'з' => latin.push('z'),
            'и' | 'й' | 'ы' => latin.push('i'),
            'к' => latin.push('k'),
            'л' => latin.push('l'),
            'м' => latin.push('m'),
            'н' => latin.push('n'),
            'о' => latin.push('o'),
            'п' => latin.push('p'),
            'р' => latin.push('r'),
            'с' => latin.push('s'),
            'т' => latin.push('t'),
            'у' => latin.push('u'),
            'ф' => latin.push('f'),
            'х' => latin.push('h'),
            'ц' => latin.push_str("ts"),
            'ч' => latin.push('C'),
            'ш' | 'щ' => latin.push('S'),
            'ю' => latin.push_str("iu"),
            'я' => latin.push_str("ia"),
            'ь' | 'ъ' => {}
            other => latin.push(other),
        }
    }
    let mut s = latin;
    for (from, to) in [
        ("dzh", "J"),
        ("tch", "C"),
        ("ch", "C"),
        ("sh", "S"),
        ("zh", "J"),
        ("dj", "J"),
        ("dJ", "J"),
        ("ph", "f"),
        ("th", "t"),
        ("ck", "k"),
        ("qu", "kv"),
        ("oo", "u"),
        ("ee", "i"),
        ("ou", "u"),
    ] {
        s = s.replace(from, to);
    }
    let chars: Vec<char> = s.chars().collect();
    let mut out = String::with_capacity(chars.len());
    for (i, &c) in chars.iter().enumerate() {
        let next = chars.get(i + 1).copied();
        match c {
            'c' if matches!(next, Some('e' | 'i' | 'y')) => out.push('s'),
            'c' | 'q' => out.push('k'),
            'x' => out.push_str("ks"),
            'w' => out.push('v'),
            'y' => out.push('i'),
            'j' => out.push('J'),
            other => out.push(other),
        }
    }
    let mut folded: Vec<char> = Vec::with_capacity(out.len());
    for c in out.chars() {
        if folded.last() != Some(&c) {
            folded.push(c);
        }
    }
    if folded.len() > 3 && folded.last() == Some(&'e') {
        folded.pop();
    }
    folded.into_iter().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixed(text: &str) -> String {
        TermDictionary::builtin().correct(text).0
    }

    #[test]
    fn repairs_transliterations_and_misspellings() {
        for (heard, expected) in [
            ("давай в гугл мит", "давай в Google Meet"),
            ("Google Mit в десять", "Google Meet в десять"),
            ("гугл мит", "Google Meet"),
            ("заведи тикет в джире", "заведи тикет в Jira"),
            ("заведи тикет в джира", "заведи тикет в Jira"),
            ("макет в фигме", "макет в Figma"),
            ("залей в гитхаб", "залей в GitHub"),
            ("запиши в конфлюенс", "запиши в Confluence"),
            ("созвон в зуме", "созвон в Zoom"),
            ("напиши в телеграмм", "напиши в Telegram"),
            ("в слаке", "в Slack"),
            ("в слак", "в Slack"),
            ("на ios и macos", "на iOS и macOS"),
            ("на айос", "на iOS"),
            ("вызови апи", "вызови API"),
            ("новый api и sdk", "новый API и SDK"),
            ("это mvp по okr", "это MVP по OKR"),
            ("глянь андроид и виндовс", "глянь Android и Windows"),
            ("запусти bakend и frontend", "запусти backend и frontend"),
            ("Deploi завтра", "Deploy завтра"),
            ("Release готов", "Release готов"),
            ("нужен аб тест", "нужен A/B-тест"),
        ] {
            assert_eq!(fixed(heard), expected, "{heard}");
        }
    }

    #[test]
    fn records_corrections_and_is_idempotent() {
        let dict = TermDictionary::builtin();
        let (text, corrections) = dict.correct("встреча в гугл мит и джире");
        assert_eq!(text, "встреча в Google Meet и Jira");
        assert_eq!(
            corrections,
            vec![
                TextCorrection {
                    original: "гугл мит".into(),
                    replacement: "Google Meet".into()
                },
                TextCorrection {
                    original: "джире".into(),
                    replacement: "Jira".into()
                },
            ]
        );
        assert!(dict.correct(&text).1.is_empty());
    }

    #[test]
    fn matches_whole_tokens_only() {
        assert_eq!(fixed("апиарий и слакер"), "апиарий и слакер");
        assert_eq!(
            fixed("zoomer, apiary, miroslav"),
            "zoomer, apiary, miroslav"
        );
    }

    #[test]
    fn jargon_is_only_repaired_in_latin_and_keeps_its_capital() {
        assert_eq!(
            fixed("релиз и деплой на бэкенд"),
            "релиз и деплой на бэкенд"
        );
        assert_eq!(fixed("Frontend упал"), "Frontend упал");
        assert_eq!(fixed("fronted упал"), "frontend упал");
    }

    #[test]
    fn ambiguous_english_words_stay_in_english_text() {
        assert_eq!(
            fixed("the notion of windows and slack"),
            "the notion of windows and slack"
        );
        assert_eq!(fixed("открой windows и notion"), "открой Windows и Notion");
    }

    #[test]
    fn user_terms_extend_the_dictionary() {
        let dict = TermDictionary::with_user_terms(["Kafka", "Битрикс24", "SLA"]);
        assert_eq!(dict.correct("поднял кафка").0, "поднял Kafka");
        assert_eq!(dict.correct("sla нарушен").0, "SLA нарушен");
        assert_eq!(dict.correct("кафе рядом").0, "кафе рядом");
    }

    #[test]
    fn segment_keeps_a_record() {
        let mut seg = Segment {
            start_ms: 0,
            end_ms: 1000,
            track: yc_core::meeting::TrackKind::Mic,
            speaker: "me".into(),
            text: "открой джира".into(),
            words: Vec::new(),
            confidence: None,
            corrections: Vec::new(),
        };
        TermDictionary::builtin().correct_segment(&mut seg);
        assert_eq!(seg.text, "открой Jira");
        assert_eq!(seg.corrections.len(), 1);
    }

    /// Everyday Russian and English that must come out byte-for-byte unchanged.
    const ORDINARY: &[&str] = &[
        "Мы обсудили план релиза на следующей неделе.",
        "Пришла телеграмма от бабушки.",
        "Я купил новую гитару и играю каждый вечер.",
        "Давай созвонимся завтра в десять утра.",
        "Мой друг Миша живёт в Москве.",
        "Скачай документ и отправь мне на почту.",
        "Это слабое место нашей архитектуры.",
        "Тестирование показало несколько ошибок в интерфейсе.",
        "Нужно подготовить презентацию для клиента.",
        "Зуб болит уже третий день.",
        "В кафе рядом с офисом очень вкусный кофе.",
        "Он быстро набрал номер и подождал ответа.",
        "Мирный договор подписали в конце весны.",
        "Мы с командой делаем приложение для звонков.",
        "Где-то в середине года поменяем подход.",
        "Лаборатория работает над новым проектом.",
        "Правила игры понятны всем участникам.",
        "Вечером поедем за город встречать рассвет.",
        "Сколько времени займёт перенос данных?",
        "Тимлид пересмотрел задачи на спринт.",
        "Обязательно напиши мне, когда будешь дома.",
        "Конференция пройдёт в Санкт-Петербурге.",
        "Они договорились встретиться возле метро.",
        "Какая сегодня погода, будет ли дождь?",
        "Я думаю, что гитарист сыграл отлично.",
        "Слушай, у меня плохая связь, перезвоню позже.",
        "Мы вчера смотрели фильм про космос.",
        "Закажи пиццу и два салата на всех.",
        "Дима просил прислать отчёт до пятницы.",
        "Фигурное катание началось в восемь вечера.",
        "Вилка и ложка лежат на столе.",
        "Командир отряда дал сигнал к отходу.",
        "Let us meet tomorrow and review the schedule.",
        "The team shipped a small change to the checkout flow.",
        "Please send the document before the call starts.",
        "Zero downtime is not realistic for this project.",
    ];

    #[test]
    fn ordinary_sentences_are_untouched() {
        assert!(ORDINARY.len() >= 30);
        let dict = TermDictionary::builtin();
        for sentence in ORDINARY {
            let (text, corrections) = dict.correct(sentence);
            assert_eq!(&text, sentence);
            assert!(corrections.is_empty(), "{sentence}: {corrections:?}");
        }
    }
}
