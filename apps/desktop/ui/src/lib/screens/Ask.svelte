<script lang="ts">
  import { onMount, tick } from "svelte";
  import Icon from "../kit/Icon.svelte";
  import CopyButton from "../kit/CopyButton.svelte";
  import { api } from "../api";
  import type { Citation, ModelEntry, SentNote } from "../api";
  import { t, plural } from "../i18n/index.svelte";
  import { clock, shortDate } from "../format";
  import { go, back, href } from "../router.svelte";
  import { speakerName } from "../speakers";
  import { errText } from "../api/error";
  import { onEngineEvent } from "../state.svelte";

  let { q }: { q?: string } = $props();

  type QA = { q: string; text: string; citations: Citation[]; sent?: SentNote; busy: boolean; error?: string; picked: number | null };
  let thread = $state<QA[]>([]);
  let input = $state("");
  let scroller: HTMLElement;

  async function ask(question: string) {
    thread.push({ q: question, text: "", citations: [], busy: true, picked: null });
    const qa = thread[thread.length - 1];
    await tick(); scroller?.scrollTo({ top: scroller.scrollHeight, behavior: "smooth" });
    try {
      await api.ask(question, (e) => {
        if (e.kind === "token") qa.text += e.text;
        else if (e.kind === "citations") qa.citations = e.items;
        else if (e.kind === "done") { qa.sent = e.sent; qa.busy = false; }
        else if (e.kind === "error") { qa.error = t(e.message); qa.busy = false; }
      });
    } catch (err) { qa.error = errText(err); }
    qa.busy = false;
  }
  onMount(() => { if (q) ask(q); });

  // The search model is fetched in the background after setup; until it is there, search matches words only.
  let embed = $state<ModelEntry | null>(null);
  async function loadEmbed() { try { embed = (await api.modelsOverview()).models.find((m) => m.kind === "embed" && m.active) ?? null; } catch { embed = null; } }
  onMount(() => {
    loadEmbed();
    return onEngineEvent((e) => {
      if (e.type !== "download" || !embed || e.item !== embed.id) return;
      if (e.state.state === "done") loadEmbed();
      else embed.download = { state: e.state, doneBytes: e.doneBytes, totalBytes: e.totalBytes, bytesPerSec: e.bytesPerSec };
    });
  });
  const embedPct = $derived(embed?.download ? Math.floor((embed.download.doneBytes / Math.max(1, embed.download.totalBytes)) * 100) : null);

  function submit(e: SubmitEvent) {
    e.preventDefault();
    const v = input.trim();
    if (!v) return;
    input = ""; ask(v);
  }
  function segments(text: string): ({ s: string } | { n: number })[] {
    const out: ({ s: string } | { n: number })[] = [];
    let p = 0;
    for (const m of text.matchAll(/\s?\[(\d+)\]/g)) {
      if (m.index! > p) out.push({ s: text.slice(p, m.index) });
      out.push({ n: Number(m[1]) });
      p = m.index! + m[0].length;
    }
    if (p < text.length) out.push({ s: text.slice(p) });
    return out;
  }
  const who = (c: Citation) => (c.speaker === "me" ? t("Вы") : c.speaker);
</script>

<main>
  <div class="yc-scroll" style="flex: 1; min-height: 0" bind:this={scroller}>
    <div class="yc-in col">
      <button type="button" class="backlink" onclick={() => back()}><Icon name="back" size={14} />{t("Встречи")}</button>

      {#if embed && !embed.installed}
        <p class="hint" role="status">
          <Icon name="search" size={14} />
          <span>
            {#if embed.download && ["queued", "running", "verifying"].includes(embed.download.state.state)}{t("Скачиваю модель смыслового поиска: {p}%. Пока ищу только по словам.", { p: embedPct ?? 0 })}
            {:else}{t("Смысловой поиск выключен: модель не скачана, ищу только по словам.")} <a href={href({ name: "models" })}>{t("Скачать")}</a>{/if}
          </span>
        </p>
      {/if}

      {#each thread as qa, qi (qi)}
        <div style="display: flex; justify-content: flex-end; padding-left: 64px">
          <div class="ubub">{qa.q}</div>
        </div>
        <article>
          {#if qa.error}
            <p class="ans" style="color: var(--danger-text)">{qa.error}</p>
          {:else}
            <p class="yc-pretty ans" class:yc-caret={qa.busy}>
              {#each segments(qa.text) as seg, si (si)}
                {#if "s" in seg}{seg.s}{:else}{" "}<button type="button" class="yc-tc mg-mono tcb" aria-label={t("Источник {n}", { n: seg.n })} onclick={() => (qa.picked = qa.picked === seg.n ? null : seg.n)}>{seg.n}</button>{/if}
              {/each}
              {#if qa.busy && !qa.text}<span style="color: var(--muted)">{t("Ищу по всем встречам…")}</span>{/if}
            </p>
          {/if}

          {#if qa.citations.length}
            <div class="srcs">
              {#each qa.citations as c, ci (c.n)}
                <a class="yc-row src {qa.picked === c.n ? 'yc-bub-on' : ''}" style:border-top-color={ci === 0 ? "transparent" : "var(--border)"} href={href({ name: "meeting", id: c.meetingId, tab: "transcript", at: c.atMs })}>
                  <span class="mg-mono n">{c.n}</span>
                  <span class="mid">
                    <span style="font-size: 13px; font-weight: 700">{c.meetingTitle} <span class="mg-mono" style="font-weight: 500; color: var(--muted)">· {shortDate(c.startedAt)}</span></span>
                    <span class="yc-ellipsis" style="font-size: 13px; color: var(--muted)">{who(c)}: «{c.quote}»</span>
                  </span>
                  <span class="mg-mono at">▸ {clock(c.atMs)}</span>
                </a>
              {/each}
            </div>
          {/if}

          {#if !qa.busy && qa.text}
            <div class="foot">
              <CopyButton value={qa.text.replace(/\s?\[\d+\]/g, "")} label={t("Копировать")} />
              <span style="flex: 1"></span>
              {#if qa.sent}
                <span class="sent"><Icon name="lock" size={12} />
                  {#if qa.sent.excerpts === 0 || qa.sent.provider === "local"}{t("Всё осталось на компьютере — ничего не отправлялось")}
                  {:else}{t("В {provider} ушли {n} {w}, аудио — нет", { provider: qa.sent.provider, n: qa.sent.excerpts, w: plural(qa.sent.excerpts, ["отрывок текста", "отрывка текста", "отрывков текста"], ["text excerpt", "text excerpts"]) })}{/if}
                </span>
              {/if}
            </div>
          {/if}
        </article>
      {/each}
    </div>
  </div>

  <div aria-hidden="true" class="fade"></div>
  <form class="askbar" onsubmit={submit}>
    <label style="flex: 1; min-width: 0; display: flex"><span class="mg-sr">{t("Уточнить вопрос")}</span>
      <input type="text" bind:value={input} placeholder={thread.length ? t("А что на это сказал Борис?") : t("Какие задачи на мне на этой неделе?")} /></label>
    <button type="submit" aria-label={t("Спросить")} class="yc-press send"><Icon name="arrowUp" size={16} /></button>
  </form>
</main>

<style>
  main { flex: 1 1 520px; min-width: 0; display: flex; flex-direction: column; min-height: 0; position: relative; }
  .col { max-width: 640px; box-sizing: border-box; margin: 0 auto; padding: 22px 28px 130px; display: flex; flex-direction: column; gap: 20px; }
  .backlink { align-self: flex-start; display: inline-flex; align-items: center; gap: 6px; padding: 0; border: 0; background: none; font-size: 13px; color: var(--muted); }
  .hint { margin: 0; display: flex; align-items: flex-start; gap: 8px; padding: 9px 12px; border: 1px solid var(--border); border-radius: 12px; background: var(--surface); font-size: 12px; line-height: 1.5; color: var(--muted); }
  .hint a { color: var(--accent-text); }
  .ubub { padding: 10px 14px; border-radius: 18px 18px 6px 18px; background: var(--accent-soft); font-size: 15px; line-height: 1.5; }
  article { display: flex; flex-direction: column; gap: 14px; }
  .ans { margin: 0; font-size: 15px; line-height: 1.7; }
  .tcb { display: inline-grid; place-items: center; min-width: 18px; height: 18px; padding: 0 4px; margin: 0 1px; border: 1px solid var(--border); border-radius: 6px; background: var(--surface-2); color: var(--accent-text); font-size: 10px; font-weight: 700; vertical-align: 2px; }
  .srcs { display: flex; flex-direction: column; border: 1px solid var(--border); border-radius: 16px; background: var(--surface); overflow: hidden; }
  .src { display: flex; gap: 12px; align-items: baseline; padding: 11px 14px; border: 1px solid transparent; color: var(--text); text-decoration: none; }
  .n { flex: none; font-size: 11px; font-weight: 700; color: var(--muted); }
  .mid { flex: 1; min-width: 0; display: flex; flex-direction: column; gap: 2px; }
  .at { flex: none; font-size: 11px; color: var(--accent-text); }
  .foot { display: flex; flex-wrap: wrap; align-items: center; gap: 8px 12px; }
  .sent { display: inline-flex; align-items: center; gap: 6px; font-size: 12px; color: var(--muted); }
  .fade { position: absolute; left: 0; right: 10px; bottom: 0; height: 110px; pointer-events: none; background: linear-gradient(to bottom, transparent, var(--bg) 60%); }
  .askbar { position: absolute; left: 28px; right: 28px; bottom: 18px; max-width: 584px; margin: 0 auto; display: flex; align-items: center; gap: 10px; height: 48px; box-sizing: border-box; padding: 0 5px 0 18px; border: 1px solid var(--border); border-radius: 24px; background: var(--dock); backdrop-filter: blur(16px); -webkit-backdrop-filter: blur(16px); box-shadow: var(--shadow-dock); }
  input { flex: 1; min-width: 0; height: 36px; margin: 0; padding: 0; border: 0; background: transparent; color: var(--text); font: inherit; font-size: 14px; outline-offset: 6px; }
  .send { flex: none; width: 36px; height: 36px; border: 0; border-radius: 50%; background: var(--surface-2); color: var(--muted); display: grid; place-items: center; }
</style>
