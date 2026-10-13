/**
 * Salient words of a meeting for the processing screen: names, terms, Latin words and rare long words from the
 * transcript the UI already has. Everything stays in memory in the webview (nothing is sent anywhere).
 */

const STOP = new Set(`а б в г д е ж з и к л м н о п р с т у ф х ц ч ш щ э ю я
без более бы был была были было быть вам вас ведь весь вот все всё вы где да даже для до его ее её если есть еще ещё же за здесь из или им их как какая какой когда который кто ли либо мне может можно мой мы на над надо нас не него нет ни них но ну о об однако он она они оно от очень по под после потому потом при про раз с сам свой себя со так также такой там те тебе тем то тогда того той только том тоже ты тут у уже хотя чего чем что чтобы эта эти это этого этой этом этот я
ну вот типа короче ага угу мм ммм эм эээ ээ ладно давай давайте слушай смотри значит просто конечно наверное вообще
a an and are as at be but by for from has have he her him his i if in is it its me my no not of on or our she so that the their them then there they this to was we were what when which who will with you your
ok okay yeah yes um uh hmm like just well right`.split(/\s+/));

export interface Candidate { word: string; score: number }

const WORD = /[\p{L}][\p{L}\p{N}'’-]*/gu;
const isLatin = (w: string) => /^[A-Za-z][A-Za-z0-9'’-]*$/.test(w);

export class WordFeed {
  private cand = new Map<string, { word: string; base: number; count: number }>();
  private used = new Set<string>();

  private letters = new Map<string, number>();

  clear() { this.cand.clear(); this.used.clear(); this.letters.clear(); }

  /** Letters of the meeting, most frequent first: the glyph field is made of them. */
  alphabet(): string {
    return [...this.letters.entries()].sort((a, b) => b[1] - a[1]).slice(0, 36).map(([c]) => c).join("");
  }
  get size() { return this.cand.size; }

  /** Adds the words of one phrase; `terms` are dictionary hits (replacement texts) that always qualify. */
  add(text: string, terms: string[] = []) {
    for (const ch of text.toLowerCase()) if (/\p{L}/u.test(ch)) this.letters.set(ch, (this.letters.get(ch) ?? 0) + 1);
    for (const t of terms) this.put(t.trim(), 6);
    let first = true;
    for (const m of text.matchAll(WORD)) {
      const w = m[0].replace(/^[-'’]+|[-'’]+$/g, "");
      const lead = first; first = false;
      if (w.length < 3) continue;
      const low = w.toLowerCase();
      if (STOP.has(low)) continue;
      let base = 0;
      const capital = /^\p{Lu}/u.test(w) && !/^\p{Lu}+$/u.test(w);
      if (capital && !lead) base = 3;             // a name in the middle of a sentence
      else if (isLatin(w)) base = 2.5;            // product names, English words
      else if (w.length >= 6) base = 1 + Math.min(1.5, (w.length - 6) * 0.25);
      if (base > 0) this.put(w, base);
    }
  }

  private put(word: string, base: number) {
    if (!word || /\d/.test(word) || word.length < 3) return;
    const key = word.toLowerCase();
    if (STOP.has(key)) return;
    const c = this.cand.get(key);
    if (c) { c.count++; c.base = Math.max(c.base, base); } else this.cand.set(key, { word, base, count: 1 });
  }

  /** A salient word (or short phrase) that fits `maxLen` cells; no repeats until every candidate was shown. */
  next(maxLen: number): string | null {
    const pool = [...this.cand.entries()].filter(([, c]) => c.word.length <= maxLen);
    if (!pool.length) return null;
    let fresh = pool.filter(([k]) => !this.used.has(k));
    if (!fresh.length) { for (const [k] of pool) this.used.delete(k); fresh = pool; }
    // Repeated words are more likely the topic than a slip: weight by base score plus a little frequency.
    const weighted = fresh.map(([k, c]) => ({ k, c, w: c.base + Math.min(c.count, 4) * 0.6 }));
    weighted.sort((a, b) => b.w - a.w);
    const top = weighted.slice(0, 12);
    const total = top.reduce((s, x) => s + x.w, 0);
    let r = Math.random() * total;
    const pick = top.find((x) => (r -= x.w) <= 0) ?? top[0];
    this.used.add(pick.k);
    return pick.c.word;
  }
}

/** The meeting being recorded / processed: filled from live segments, read by the processing visualiser. */
export const meetingWords = new WordFeed();
