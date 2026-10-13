// Strings that are NOT in the Claude Design export: SEO, the "What's new" pill, 404, privacy pages.
import type { Lang } from './i18n';

export interface Extra {
  locale: string; // BCP-47 for hreflang
  ogLocale: string;
  siteName: string;
  title: string;
  description: string;
  whatsNew: string;
  ossLink: string;
  skip: string;
  privacyTitle: string;
  privacyDescription: string;
  back: string;
  notFoundTitle: string;
  notFoundText: string;
  notFoundHome: string;
}

export const X: Record<Lang, Extra> = {
  ru: {
    locale: 'ru',
    ogLocale: 'ru_RU',
    siteName: 'YadaCall',
    title: 'YadaCall — созвоны в текст прямо на компьютере',
    description:
      'YadaCall пишет тебя и собеседников двумя дорожками и расшифровывает на твоём компьютере. Звук никуда не уходит: ни бота в звонке, ни аккаунта, ни телеметрии.',
    whatsNew: 'Что нового',
    ossLink: 'Open source',
    skip: 'К содержимому',
    privacyTitle: 'Политика конфиденциальности — YadaCall',
    privacyDescription:
      'Что YadaCall делает с вашими данными: звук и расшифровки остаются на компьютере, данные Google Календаря читаются только для чтения и не покидают устройство, телеметрии и аналитики нет.',
    back: 'На главную',
    notFoundTitle: 'Страница не найдена',
    notFoundText: 'Такой страницы нет. Возможно, ссылка устарела.',
    notFoundHome: 'На главную',
  },
  en: {
    locale: 'en',
    ogLocale: 'en_US',
    siteName: 'YadaCall',
    title: 'YadaCall — turn calls into text on your own computer',
    description:
      'YadaCall records you and the other side on two separate tracks and transcribes on your own computer. Audio never leaves it: no bot in the call, no account, no telemetry.',
    whatsNew: 'What’s new',
    ossLink: 'Open source',
    skip: 'Skip to content',
    privacyTitle: 'Privacy policy — YadaCall',
    privacyDescription:
      'What YadaCall does with your data: audio and transcripts stay on your computer, Google Calendar data is read-only and never leaves your device, and there is no telemetry or analytics.',
    back: 'Back to the homepage',
    notFoundTitle: 'Page not found',
    notFoundText: 'There is no such page. The link may be out of date.',
    notFoundHome: 'Back to the homepage',
  },
};

export interface PrivacySection {
  id: string;
  h: string;
  /** trusted, authored HTML */
  html: string;
}

export interface PrivacyDoc {
  title: string;
  updated: string;
  updatedIso: string;
  lead: string;
  tocLabel: string;
  sections: PrivacySection[];
}

const LIMITED_USE =
  'YadaCall’s use and transfer to any other app of information received from Google APIs will adhere to the <a href="https://developers.google.com/terms/api-services-user-data-policy">Google API Services User Data Policy</a>, including the Limited Use requirements.';

export const PRIVACY: Record<Lang, PrivacyDoc> = {
  ru: {
    title: 'Политика конфиденциальности',
    updated: 'Действует с 13 октября 2026',
    updatedIso: '2026-10-13',
    lead: 'Коротко: YadaCall — программа для вашего компьютера. Звук и расшифровки не покидают его, у нас нет сервера и аккаунтов, а данные Google Календаря приложение только читает и хранит у вас на устройстве.',
    tocLabel: 'Содержание',
    sections: [
      {
        id: 'who',
        h: 'Кто мы',
        html: `
<p>YadaCall — бесплатное приложение для записи и расшифровки созвонов с открытым исходным кодом (лицензия MIT). Его разрабатывает проект <a href="https://github.com/limeflash/yadacall">limeflash/yadacall</a> на GitHub; под «мы» в этом тексте понимается он.</p>
<p>У приложения <strong>нет серверов и нет аккаунтов</strong>. Мы не получаем ваши записи, тексты, имена, адреса почты или что-либо ещё, что вы делаете в приложении: всё это остаётся на вашем компьютере.</p>`,
      },
      {
        id: 'audio',
        h: 'Звук и расшифровки',
        html: `
<ul>
<li>Звук записывается и обрабатывается <strong>только на вашем компьютере</strong>: расшифровку делают модели, которые работают локально и без интернета.</li>
<li>Встречи хранятся обычными файлами в выбранной вами папке: расшифровка (<code>transcript.md</code>, <code>transcript.json</code>), итоги и, если вы так решили, звук в формате FLAC.</li>
<li>Срок хранения звука настраивается: не хранить, 7 дней, 30 дней или всегда. Текст и итоги остаются, пока вы сами их не удалите.</li>
<li>Мы никогда не получаем ни звук, ни расшифровки. Их нельзя запросить у нас, потому что у нас их нет.</li>
</ul>`,
      },
      {
        id: 'network',
        h: 'Что уходит в сеть',
        html: `
<p>Приложение обращается к интернету только в этих случаях:</p>
<ul>
<li><strong>Загрузка моделей</strong> (Hugging Face и зеркало hf-mirror) и <strong>обновлений приложения</strong> (GitHub). В таких запросах нет вашего звука и текстов; серверы, как и любой сайт, видят IP-адрес запроса.</li>
<li><strong>Google Календарь</strong> — только если вы сами подключили аккаунт Google (подробности в разделе «Данные Google»).</li>
<li><strong>Календарь по ICS-ссылке</strong> — если вы добавили такую ссылку, приложение скачивает по ней события.</li>
<li><strong>Облачный провайдер итогов</strong> — только если вы выбрали его в настройках: Ollama (локально или Ollama Cloud), Anthropic (Claude), OpenRouter или любой OpenAI-совместимый сервер. Тогда <strong>текст</strong> расшифровки (целиком или фрагментами, например для итогов или вопросов ко встречам) отправляется выбранному провайдеру и обрабатывается по его собственной политике конфиденциальности. <strong>Звук не отправляется никогда.</strong> Ключи API хранятся в связке ключей системы (Keychain / Credential Manager). Если выбрать итоги на компьютере или «Без итогов», текст никуда не уходит.</li>
</ul>
<p>Приложение показывает, что именно ушло наружу: например, «6 фрагментов текста в Claude, аудио — нет».</p>`,
      },
      {
        id: 'google',
        h: 'Данные Google',
        html: `
<p>Вход через Google необязателен. Если вы подключаете аккаунт Google, приложение запрашивает доступ <strong>только на чтение</strong> и ровно такие разрешения (scopes):</p>
<ul>
<li><code>openid</code>, <code>email</code>, <code>profile</code> — чтобы понять, какой аккаунт подключён, и показать его имя и адрес;</li>
<li><code>https://www.googleapis.com/auth/calendar.events.readonly</code> — события ваших календарей (название, время, ссылка на встречу, участники);</li>
<li><code>https://www.googleapis.com/auth/calendar.calendarlist.readonly</code> — список ваших календарей, чтобы показать и общие, и командные.</li>
</ul>
<h3>Как мы используем эти данные</h3>
<p>Только внутри приложения и только для функций календаря: показать ближайшие встречи, напомнить о звонке за 1, 2 или 5 минут, открыть ссылку на встречу в нужном аккаунте Google и подсказать имена участников для подписей говорящих в расшифровке. Приложение ничего не создаёт, не меняет и не удаляет в вашем календаре.</p>
<h3>Где они хранятся</h3>
<ul>
<li>События календаря обрабатываются и кэшируются <strong>на вашем устройстве</strong>.</li>
<li>Токены доступа Google хранятся в связке ключей операционной системы (Keychain / Credential Manager); токен доступа держится только в памяти.</li>
<li>Эти данные <strong>не отправляются нам и никаким третьим лицам</strong>, не используются для рекламы, не продаются и не используются для обучения моделей искусственного интеллекта. Люди их не читают.</li>
</ul>
<h3>Как отозвать доступ и удалить данные</h3>
<ul>
<li>В приложении отключите аккаунт Google: токен будет отозван у Google, удалён из связки ключей, а события этого аккаунта — убраны из приложения.</li>
<li>Либо отзовите доступ на странице Google: <a href="https://myaccount.google.com/permissions">myaccount.google.com/permissions</a>.</li>
</ul>
<h3>Требования Google (Limited Use)</h3>
<p>Использование и передача любому другому приложению информации, полученной из API Google, соответствуют <a href="https://developers.google.com/terms/api-services-user-data-policy">Политике в отношении пользовательских данных в сервисах API Google</a>, включая требования об ограниченном использовании. В оригинале это звучит так:</p>
<p lang="en">«${LIMITED_USE}»</p>`,
      },
      {
        id: 'tracking',
        h: 'Телеметрия, аналитика и cookies',
        html: `
<p>В приложении <strong>нет телеметрии, аналитики, отчётов о сбоях и трекеров</strong>.</p>
<p>На этом сайте тоже нет cookies, счётчиков, рекламы и сторонних скриптов или шрифтов. Единственный сетевой запрос сайта — к <code>api.github.com</code>, чтобы показать актуальную версию и ссылки на скачивание; ответ запоминается в браузере на час. Сам сайт размещён на GitHub Pages — GitHub, как любой хостинг, может вести собственные журналы запросов по своим правилам.</p>`,
      },
      {
        id: 'children',
        h: 'Дети',
        html: `<p>YadaCall не предназначен для детей и не собирает данные ни о ком, в том числе о детях: у нас нет данных пользователей.</p>`,
      },
      {
        id: 'changes',
        h: 'Изменения политики',
        html: `<p>Если политика изменится, новая редакция появится на этой странице с новой датой, а история изменений доступна в <a href="https://github.com/limeflash/yadacall">репозитории проекта</a>. Существенные изменения мы также отметим в заметках к релизу.</p>`,
      },
      {
        id: 'contact',
        h: 'Контакты',
        html: `<p>Вопросы, замечания и запросы по приватности — в <a href="https://github.com/limeflash/yadacall/issues">GitHub Issues</a> проекта.</p>`,
      },
    ],
  },
  en: {
    title: 'Privacy policy',
    updated: 'Effective 13 October 2026',
    updatedIso: '2026-10-13',
    lead: 'In short: YadaCall is a program for your computer. Audio and transcripts never leave it, we have no server and no accounts, and the app only reads Google Calendar data and keeps it on your device.',
    tocLabel: 'Contents',
    sections: [
      {
        id: 'who',
        h: 'Who we are',
        html: `
<p>YadaCall is a free, open-source (MIT) desktop app that records and transcribes calls. It is developed by the GitHub project <a href="https://github.com/limeflash/yadacall">limeflash/yadacall</a>; “we” in this text means that project.</p>
<p>The app has <strong>no servers and no accounts</strong>. We do not receive your recordings, transcripts, names, email addresses or anything else you do in the app: all of it stays on your computer.</p>`,
      },
      {
        id: 'audio',
        h: 'Audio and transcripts',
        html: `
<ul>
<li>Audio is recorded and processed <strong>only on your computer</strong>: transcription is done by models that run locally, with no internet needed.</li>
<li>Meetings are stored as plain files in a folder you choose: the transcript (<code>transcript.md</code>, <code>transcript.json</code>), the summary and, if you choose to keep it, the audio as FLAC.</li>
<li>Audio retention is a setting: don’t keep, 7 days, 30 days or forever. Text and summaries stay until you delete them.</li>
<li>We never receive your audio or transcripts. There is nothing to request from us because we do not have it.</li>
</ul>`,
      },
      {
        id: 'network',
        h: 'What goes over the network',
        html: `
<p>The app connects to the internet only in these cases:</p>
<ul>
<li><strong>Downloading models</strong> (Hugging Face and the hf-mirror mirror) and <strong>app updates</strong> (GitHub). These requests contain none of your audio or text; the servers, like any website, see the IP address of the request.</li>
<li><strong>Google Calendar</strong> — only if you connected a Google account yourself (see “Google user data” below).</li>
<li><strong>A calendar via ICS link</strong> — if you added one, the app downloads its events from that link.</li>
<li><strong>A cloud summary provider</strong> — only if you pick one in settings: Ollama (local or Ollama Cloud), Anthropic (Claude), OpenRouter or any OpenAI-compatible server. In that case the <strong>text</strong> of the transcript (in full or in fragments, for example for summaries or questions across meetings) is sent to the provider you chose and handled under that provider’s own privacy policy. <strong>Audio is never sent.</strong> API keys are kept in the system keychain (Keychain / Credential Manager). If you choose on-device summaries or “No summaries”, no text leaves your computer.</li>
</ul>
<p>The app shows exactly what went out, for example “6 text fragments to Claude, no audio.”</p>`,
      },
      {
        id: 'google',
        h: 'Google user data',
        html: `
<p>Signing in with Google is optional. If you connect a Google account, the app asks for <strong>read-only</strong> access and exactly these permissions (scopes):</p>
<ul>
<li><code>openid</code>, <code>email</code>, <code>profile</code> — to know which account is connected and to show its name and address;</li>
<li><code>https://www.googleapis.com/auth/calendar.events.readonly</code> — events in your calendars (title, time, meeting link, attendees);</li>
<li><code>https://www.googleapis.com/auth/calendar.calendarlist.readonly</code> — the list of your calendars, so shared and team calendars show up too.</li>
</ul>
<h3>How we use it</h3>
<p>Only inside the app and only for calendar features: to show your upcoming meetings, remind you 1, 2 or 5 minutes before a call, open the meeting link in the right Google account, and suggest participant names for speaker labels in the transcript. The app never creates, edits or deletes anything in your calendar.</p>
<h3>Where it is stored</h3>
<ul>
<li>Calendar events are processed and cached <strong>on your device</strong>.</li>
<li>Google sign-in tokens are stored in the operating system keychain (Keychain / Credential Manager); the access token is kept in memory only.</li>
<li>This data is <strong>never sent to us or to any third party</strong>, never used for advertising, never sold, and never used to train AI models. No humans read it.</li>
</ul>
<h3>How to revoke access and delete the data</h3>
<ul>
<li>In the app, disconnect the Google account: the token is revoked at Google, removed from the keychain, and the account’s events are dropped from the app.</li>
<li>Or revoke access on Google’s side: <a href="https://myaccount.google.com/permissions">myaccount.google.com/permissions</a>.</li>
</ul>
<h3>Google API Services User Data Policy (Limited Use)</h3>
<p>${LIMITED_USE}</p>`,
      },
      {
        id: 'tracking',
        h: 'Telemetry, analytics and cookies',
        html: `
<p>The app has <strong>no telemetry, analytics, crash reporting or trackers</strong>.</p>
<p>This website has none either: no cookies, no counters, no ads, no third-party scripts or fonts. The site’s only network request goes to <code>api.github.com</code> to show the current version and download links; the answer is remembered in your browser for an hour. The site is hosted on GitHub Pages — GitHub, like any host, may keep its own request logs under its own rules.</p>`,
      },
      {
        id: 'children',
        h: 'Children',
        html: `<p>YadaCall is not directed at children and does not collect data about anyone, children included: we hold no user data at all.</p>`,
      },
      {
        id: 'changes',
        h: 'Changes to this policy',
        html: `<p>If the policy changes, the new version will be published on this page with a new date, and the history is available in the <a href="https://github.com/limeflash/yadacall">project repository</a>. We will also mention significant changes in the release notes.</p>`,
      },
      {
        id: 'contact',
        h: 'Contact',
        html: `<p>Questions, remarks and privacy requests: please open an issue in the project’s <a href="https://github.com/limeflash/yadacall/issues">GitHub Issues</a>.</p>`,
      },
    ],
  },
};
