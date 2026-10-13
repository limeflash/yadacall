// Copy of the approved Claude Design strings (T / LINES / CALLS / TECH), kept verbatim.
// Only additions live in `X` below (SEO, privacy links, "What's new" pill, 404).

export type Lang = 'ru' | 'en';

export interface Line {
  me: boolean;
  name: string;
  text: string;
}

export interface Dict {
  navLabel: string; navFeatures: string; navPrivacy: string; navModels: string; navDownload: string; langLabel: string;
  release: string;
  h1: [string, number][];
  sub: string;
  ctaFor: string; cta2: string;
  checks: string[];
  meta: string;
  me: string; them: string;
  shotLabel: string; altRec: string;
  liveLabel: string; liveName: string; liveText: string;
  lockTitle: string; lockSub: string;
  callsLabel: string; callsIntro: string; callsNote: string;
  statsLabel: string; mb: string;
  stats: string[];
  trEyebrow: string; trTitle: string; trText: string;
  speed: { v: string; l: string; n: string }[];
  demoLabel: string; demoTitle: string; legendMe: string; legendThem: string; mic: string; callAudio: string; onDevice: string;
  whoEyebrow: string; whoTitle: string; whoText: string;
  keyMark: string; keyRec: string; altTr: string;
  sumEyebrow: string; sumTitle: string; sumText: string; altSum: string;
  whereTitle: string; offlineTag: string;
  where: [string, string][];
  cloudNote: string;
  askEyebrow: string; askTitle: string; askText: string; altAsk: string;
  calEyebrow: string; calTitle: string; calText: string; altCal: string;
  small: [string, string, string][];
  modEyebrow: string; modTitle: string; modText: string;
  dlChips: string[];
  models: [string, string, string, string][];
  measured: string;
  privEyebrow: string; privTitle: string; privText: string;
  stayTitle: string; goTitle: string;
  stay: string[]; go: string[];
  ossTitle: string; ossText: string; hood: string; ossModels: string[];
  folder: string; f1: string; f2: string; f3: string; fSummary: string; fAudio: string;
  keepLabel: string; keep: string[]; keepNote: string;
  limitsTitle: string;
  limits: [string, string][];
  dlTitle: string; dlText: string; yours: string; btn: string;
  req: { mac: string; win: string; linux: string };
  allVersions: string;
  footer: string; footLinks: string; releases: string; policy: string;
}

const LINES_RAW = {
  ru: [
    { me: false, name: 'Аня', text: 'По срокам всё упирается в прогресс онбординга — без него не помним, где человек остановился.' },
    { me: true, name: 'Я', text: 'Тогда двигаем релиз на двадцать четвёртое. Возражения есть?' },
    { me: false, name: 'Борис', text: 'Мне так даже удобнее — успею собрать билд с новым порядком экранов.' },
    { me: false, name: 'Вика', text: 'Экран разрешений покажем после первого звонка. Макеты обновишь?' },
    { me: true, name: 'Я', text: 'Да, до пятнадцатого.' }
  ],
  en: [
    { me: false, name: 'Anya', text: 'It all hinges on onboarding progress — without it we can’t tell where people stopped.' },
    { me: true, name: 'Me', text: 'Then we move the release to the twenty-fourth. Any objections?' },
    { me: false, name: 'Boris', text: 'Works even better for me — I’ll have time to build the new screen order.' },
    { me: false, name: 'Vika', text: 'We’ll show the permissions screen after the first call. Can you update the mockups?' },
    { me: true, name: 'Me', text: 'Sure, by the fifteenth.' }
  ]
};
const T_RAW = {
  ru: {
    navLabel: 'Разделы', navFeatures: 'Возможности', navPrivacy: 'Приватность', navModels: 'Модели', navDownload: 'Скачать', langLabel: 'Язык',
    release: 'Первый публичный релиз',
    h1: [['Созвоны', 0], ['в', 0], ['текст —', 0], ['прямо', 1], ['во', 1], ['время', 1], ['звонка', 1]],
    sub: 'YadaCall пишет тебя и собеседников двумя дорожками и расшифровывает на твоём компьютере. Звук никуда не уходит: ни бота в звонке, ни аккаунта, ни телеметрии.',
    ctaFor: 'Скачать для ', cta2: 'Исходники на GitHub',
    checks: ['Бесплатно и open source', 'Без аккаунта', 'Работает офлайн'],
    meta: 'macOS 14.2+ на Apple Silicon · Windows 10/11 · Linux',
    me: 'Я · микрофон', them: 'Собеседники · звук звонка',
    shotLabel: 'Как выглядит приложение', altRec: 'YadaCall во время звонка: таймер, две дорожки — микрофон и звук Zoom, закладки и живая расшифровка',
    liveLabel: 'Живая расшифровка', liveName: 'Аня', liveText: 'Экран разрешений покажем после первого звонка',
    lockTitle: 'Звук остаётся здесь', lockSub: 'расшифровка на устройстве',
    callsLabel: 'С какими звонками работает', callsIntro: 'Работает с любым звонком — в браузере или в приложении', callsNote: 'В звонок ничего не подключается — собеседники ничего не видят',
    statsLabel: 'В цифрах', mb: 'МБ',
    stats: ['установщик для macOS — Rust вместо Electron', 'модель русской расшифровки, грузится за 0,6 с', 'языков распознаёт без интернета', 'аккаунтов, телеметрии и ботов в звонке'],
    trEyebrow: 'Расшифровка', trTitle: 'Текст появляется, пока вы говорите',
    trText: 'Реплики пишутся по ходу разговора. Нажал «Стоп» — расшифровка готова за секунды, а не за минуты. Всё считается на компьютере и работает без интернета.',
    speed: [
      { v: '20–26×', l: 'быстрее реального времени', n: 'русский · GigaAM v3 с пунктуацией' },
      { v: '~22×', l: 'быстрее реального времени', n: '25 европейских языков · Parakeet TDT v3' },
      { v: '2–3 мин', l: 'на час записи, если расшифровывать после', n: 'а живой текст готов сразу после «Стоп»' }
    ],
    demoLabel: 'Пример живой расшифровки', demoTitle: 'Синк по запуску онбординга', legendMe: 'я', legendThem: 'собеседники', mic: 'Микрофон', callAudio: 'Звук звонка', onDevice: 'на устройстве',
    whoEyebrow: 'Кто говорит', whoTitle: 'Сразу видно, кто что сказал',
    whoText: 'Ты и собеседники — на разных дорожках, поэтому в звонке один на один ошибок нет. В групповых голоса размечаются на устройстве, знакомых YadaCall узнаёт по прошлым встречам, а имена подсказывает приглашение в календаре.',
    keyMark: 'закладка во время звонка', keyRec: 'старт и стоп', altTr: 'Расшифровка встречи: реплики разложены по участникам, у каждой таймкод',
    sumEyebrow: 'Итоги и задачи', sumTitle: 'Договорённости и задачи — с таймкодом у каждого пункта',
    sumText: 'О чём встреча, кто что делает и к какому сроку, что осталось открытым. Клик по пункту — и ты в том моменте разговора.',
    altSum: 'Итоги встречи: о чём встреча, договорённости со временем в записи и задачи с ответственными',
    whereTitle: 'Где считать итоги — выбираешь сам', offlineTag: 'без интернета',
    where: [
      ['На компьютере', 'Gemma 4 через llama.cpp — итоги короткой встречи примерно за 40 секунд.'],
      ['Через Ollama', 'Локальные модели или облачные по подписке Ollama — за 6–10 секунд, без ключей.'],
      ['Через API', 'Claude, OpenRouter, Ollama Cloud, LM Studio или любой OpenAI-совместимый сервер.'],
      ['Без итогов', 'Только расшифровка. Шаблоны для стендапа, 1:1 и интервью — когда понадобятся.']
    ],
    cloudNote: 'В облако уходит только текст — и только если выбран облачный провайдер. Звук — никогда.',
    askEyebrow: 'Поиск и вопросы', askTitle: 'Спроси сразу у всех встреч',
    askText: 'Ответ с цитатами [1][2] — каждая ведёт в нужную встречу и момент. Поиск понимает русскую морфологию и смысл, задачи из всех встреч — на одном экране.',
    altAsk: 'Вопрос ко всем встречам: ответ с пронумерованными источниками',
    calEyebrow: 'Календарь', calTitle: 'Войти и записать — одной кнопкой',
    calText: 'Несколько аккаунтов Google сразу — рабочий и личный. Напомнит за 1, 2 или 5 минут, откроет Meet в нужном аккаунте и начнёт запись. Любой календарь — по ICS-ссылке.',
    altCal: 'Календарь: ближайший звонок с обратным отсчётом и кнопкой «Войти и записать»',
    small: [
      ['text', 'Словарь терминов', '«Google Mit» → «Google Meet», «джира» → «Jira». Встроенный список плюс свои слова.'],
      ['refresh', 'Переживёт сбой', 'Упало приложение или сменились наушники посреди звонка — теряется не больше пары секунд.'],
      ['pulse', 'AirPods не испортят звук', 'С Bluetooth-наушниками пишет встроенный микрофон — звонок не проваливается в «режим гарнитуры».'],
      ['warn', 'Скажет, если дорожка молчит', 'Не тот микрофон или звонок ушёл на телефон — предупредит сразу, а не после встречи.'],
      ['upload', 'И старые записи', 'Импорт аудио и видео популярных форматов. «Распознать заново» — новой моделью из сохранённого звука.'],
      ['bolt', 'Всё под рукой', 'Запись из трея, мини-плеер поверх окон, тёмная и светлая тема, русский и английский.']
    ],
    modEyebrow: 'Модели', modTitle: '«Потянет ли?» — ещё до скачивания',
    modText: 'YadaCall замеряет процессор, память и Neural Engine и для каждой модели заранее говорит, как она пойдёт на твоём железе. После загрузки делает реальный прогон и уточняет прогноз. Если не потянет — скажет честно.',
    dlChips: ['Загрузка в 6 потоков', 'Докачка с места обрыва', 'Проверка sha256', 'Зеркало hf-mirror'],
    models: [
      ['GigaAM v3', 'Русская расшифровка · ~230 МБ', '≈ 26× реального времени', 'ok'],
      ['Parakeet TDT v3', '25 языков', '≈ 22× реального времени', 'ok'],
      ['Gemma 4 E2B', 'Итоги на компьютере · ~2,8 ГБ', '≈ 39 ток/с · 3,5 из 16 ГБ', 'ok'],
      ['Модель покрупнее', 'Если памяти не хватит', 'Не поместится', 'warn']
    ],
    measured: 'Замеры — на MacBook Pro M1 Pro, 16 ГБ',
    privEyebrow: 'Приватность', privTitle: 'Звук никогда не покидает компьютер',
    privText: 'Без аккаунта, без телеметрии и аналитики. Приложение само показывает, что ушло наружу: «6 фрагментов текста в Claude, аудио — нет».',
    stayTitle: 'Остаётся у тебя', goTitle: 'Уходит в сеть — и только это',
    stay: ['Звук обеих дорожек — обрабатывается только на компьютере', 'Встречи — обычными файлами: расшифровка, итоги и звук', 'Ключи API и токены — в Связке ключей или Credential Manager'],
    go: ['Загрузка моделей и обновлений', 'Запросы к календарю — если подключён, только чтение', 'Текст расшифровки — только при облачном провайдере итогов'],
    ossTitle: 'Лёгкое, открытое и твоё',
    ossText: 'Rust и системный WebView — без Electron, ffmpeg и Python. Установщик весит 21 МБ, обновляется прямо в приложении. Каждая встреча — обычная папка с файлами: забери, открой в любом редакторе, экспортируй в Markdown или JSON.',
    hood: 'Под капотом',
    ossModels: ['GigaAM v3 · Сбер', 'Parakeet TDT v3 · NVIDIA', 'Silero VAD', 'CAM++ · 3D-Speaker', 'multilingual-E5', 'Gemma 4 · Google'],
    folder: '~/Созвоны', f1: 'Синк по запуску онбординга', f2: 'Стендап дизайн-команды', f3: '1:1 с Аней', fSummary: 'итоги', fAudio: 'звук · FLAC',
    keepLabel: 'Хранить звук:', keep: ['не хранить', '7 дней', '30 дней', 'всегда'], keepNote: 'Текст и итоги остаются всегда',
    limitsTitle: 'Честно о том, что пока не идеально',
    limits: [
      ['Сборки пока без подписи', 'На Mac при первом запуске — правый клик → «Открыть». На Windows — SmartScreen → «Выполнить в любом случае».'],
      ['Mac — только Apple Silicon', 'macOS 14.2 и новее. Intel пока не поддерживается.'],
      ['Вход в Google — в тестовом режиме', 'Пока Google проверяет приложение, круг аккаунтов ограничен. Календарь по ICS-ссылке работает у всех.'],
      ['Linux — Ubuntu 24.04+', 'Или другой дистрибутив такой же свежести.']
    ],
    dlTitle: 'Попробуй на следующем созвоне', dlText: 'Бесплатно. Без регистрации. Установщик — 21 МБ.', yours: 'твоя система', btn: 'Скачать',
    req: { mac: 'macOS 14.2 и новее', win: 'Windows 10 и 11', linux: 'Ubuntu 24.04 и новее' },
    allVersions: 'Все версии и заметки к релизам',
    footer: 'Записывает и расшифровывает созвоны на твоём компьютере. MIT · 2026', footLinks: 'Ссылки', releases: 'Релизы', policy: 'Политика конфиденциальности'
  },
  en: {
    navLabel: 'Sections', navFeatures: 'Features', navPrivacy: 'Privacy', navModels: 'Models', navDownload: 'Download', langLabel: 'Language',
    release: 'First public release',
    h1: [['Turn', 0], ['calls', 0], ['into', 0], ['text', 0], ['while', 1], ['you’re', 1], ['still', 1], ['talking', 1]],
    sub: 'YadaCall records you and the other side on two separate tracks and transcribes on your own computer. Audio never leaves it: no bot in the call, no account, no telemetry.',
    ctaFor: 'Download for ', cta2: 'Source on GitHub',
    checks: ['Free & open source', 'No account', 'Works offline'],
    meta: 'macOS 14.2+ on Apple Silicon · Windows 10/11 · Linux',
    me: 'Me · mic', them: 'Others · call audio',
    shotLabel: 'What the app looks like', altRec: 'YadaCall during a call: timer, two tracks — microphone and Zoom audio, bookmarks and a live transcript',
    liveLabel: 'Live transcript', liveName: 'Anya', liveText: 'We’ll show the permissions screen after the first call',
    lockTitle: 'Audio stays here', lockSub: 'transcribed on-device',
    callsLabel: 'Which calls it works with', callsIntro: 'Works with any call — in the browser or in an app', callsNote: 'Nothing joins the call — the other side sees nothing',
    statsLabel: 'In numbers', mb: 'MB',
    stats: ['macOS installer — Rust, not Electron', 'Russian speech model, loads in 0.6 s', 'languages recognized offline', 'accounts, telemetry or bots in your call'],
    trEyebrow: 'Transcription', trTitle: 'Text appears while you talk',
    trText: 'Lines show up as the conversation goes. Hit Stop and the transcript is ready in seconds, not minutes. It all runs on your computer, no internet needed.',
    speed: [
      { v: '20–26×', l: 'faster than real time', n: 'Russian · GigaAM v3 with punctuation' },
      { v: '~22×', l: 'faster than real time', n: '25 European languages · Parakeet TDT v3' },
      { v: '2–3 min', l: 'per hour of audio if transcribed afterwards', n: 'live text is ready right after Stop' }
    ],
    demoLabel: 'Live transcript example', demoTitle: 'Onboarding launch sync', legendMe: 'me', legendThem: 'others', mic: 'Microphone', callAudio: 'Call audio', onDevice: 'on device',
    whoEyebrow: 'Who said what', whoTitle: 'See who said what at a glance',
    whoText: 'You and the others are on separate tracks, so one-on-one calls never get mixed up. In group calls voices are labeled on-device, familiar voices are recognized from past meetings, and names come from the calendar invite.',
    keyMark: 'bookmark during a call', keyRec: 'start & stop', altTr: 'Meeting transcript: lines grouped by speaker, each with a timestamp',
    sumEyebrow: 'Summaries & tasks', sumTitle: 'Decisions and tasks — each with a timestamp',
    sumText: 'What the meeting was about, who does what by when, what’s still open. Click any item to jump to that moment.',
    altSum: 'Meeting summary: overview, decisions with recording times and tasks with owners',
    whereTitle: 'You choose where summaries are made', offlineTag: 'offline',
    where: [
      ['On your computer', 'Gemma 4 via llama.cpp — a short meeting’s summary in about 40 seconds.'],
      ['Via Ollama', 'Local models, or cloud models with an Ollama subscription — 6–10 seconds, no keys.'],
      ['Via API', 'Claude, OpenRouter, Ollama Cloud, LM Studio or any OpenAI-compatible server.'],
      ['No summaries', 'Transcript only. Templates for stand-ups, 1:1s and interviews when you need them.']
    ],
    cloudNote: 'Only text goes to the cloud — and only if you pick a cloud provider. Audio never does.',
    askEyebrow: 'Search & questions', askTitle: 'Ask all your meetings at once',
    askText: 'Answers come with citations [1][2], each linking to the right meeting and moment. Search understands meaning and Russian word forms; tasks from every meeting live on one screen.',
    altAsk: 'Question to all meetings: an answer with numbered sources',
    calEyebrow: 'Calendar', calTitle: 'Join and record — one button',
    calText: 'Several Google accounts at once — work and personal. A reminder 1, 2 or 5 minutes ahead opens Meet in the right account and starts recording. Any other calendar via an ICS link.',
    altCal: 'Calendar: the next call with a countdown and a Join and record button',
    small: [
      ['text', 'Term dictionary', '“Google Mit” → “Google Meet”, “jira” → “Jira”. A built-in list plus your own words.'],
      ['refresh', 'Survives crashes', 'If the app crashes or you swap headphones mid-call, you lose a couple of seconds at most.'],
      ['pulse', 'AirPods won’t ruin the audio', 'With Bluetooth headphones it records the built-in mic, so the call doesn’t drop into low-quality headset mode.'],
      ['warn', 'Warns about a silent track', 'Wrong mic, or the call moved to your phone — you’ll know right away, not after the meeting.'],
      ['upload', 'Old recordings too', 'Import audio and video in popular formats. Re-transcribe saved audio with a newer model.'],
      ['bolt', 'Everything at hand', 'Record from the tray, a mini player on top of windows, dark and light themes, English and Russian UI.']
    ],
    modEyebrow: 'Models', modTitle: '“Will it run?” — before you download',
    modText: 'YadaCall measures your CPU, memory and Neural Engine and predicts how each model will run on your hardware. After download it runs a real benchmark and refines the forecast. If it won’t run well, it tells you.',
    dlChips: ['6-stream downloads', 'Resumes after drops', 'sha256 check', 'hf-mirror'],
    models: [
      ['GigaAM v3', 'Russian transcription · ~230 MB', '≈ 26× real time', 'ok'],
      ['Parakeet TDT v3', '25 languages', '≈ 22× real time', 'ok'],
      ['Gemma 4 E2B', 'Summaries on your computer · ~2.8 GB', '≈ 39 tok/s · 3.5 of 16 GB', 'ok'],
      ['A bigger model', 'When memory runs short', 'Won’t fit', 'warn']
    ],
    measured: 'Measured on a MacBook Pro M1 Pro, 16 GB',
    privEyebrow: 'Privacy', privTitle: 'Audio never leaves your computer',
    privText: 'No account, no telemetry, no analytics. The app shows exactly what went out: “6 text fragments to Claude, no audio.”',
    stayTitle: 'Stays with you', goTitle: 'Goes online — and only this',
    stay: ['Audio from both tracks — processed only on your computer', 'Meetings — as plain files: transcript, summary and audio', 'API keys and tokens — in Keychain or Credential Manager'],
    go: ['Model and app update downloads', 'Calendar requests — if connected, read-only', 'Transcript text — only with a cloud summary provider'],
    ossTitle: 'Light, open, and yours',
    ossText: 'Rust and the system WebView — no Electron, ffmpeg or Python. The installer is 21 MB and updates itself in the app. Every meeting is a plain folder of files: take it, open it in any editor, export to Markdown or JSON.',
    hood: 'Under the hood',
    ossModels: ['GigaAM v3 · Sber', 'Parakeet TDT v3 · NVIDIA', 'Silero VAD', 'CAM++ · 3D-Speaker', 'multilingual-E5', 'Gemma 4 · Google'],
    folder: '~/Calls', f1: 'Onboarding launch sync', f2: 'Design team stand-up', f3: '1:1 with Anya', fSummary: 'summary', fAudio: 'audio · FLAC',
    keepLabel: 'Keep audio:', keep: ['don’t keep', '7 days', '30 days', 'forever'], keepNote: 'Text and summaries always stay',
    limitsTitle: 'What’s not perfect yet — honestly',
    limits: [
      ['Builds aren’t signed yet', 'On a Mac, first launch: right-click → Open. On Windows: SmartScreen → Run anyway.'],
      ['Mac: Apple Silicon only', 'macOS 14.2 or later. Intel isn’t supported yet.'],
      ['Google sign-in is in testing', 'Until Google verifies the app, only a limited set of accounts can sign in. Calendars via ICS link work for everyone.'],
      ['Linux: Ubuntu 24.04+', 'Or another distro that recent.']
    ],
    dlTitle: 'Try it on your next call', dlText: 'Free. No sign-up. 21 MB installer.', yours: 'your system', btn: 'Download',
    req: { mac: 'macOS 14.2 or later', win: 'Windows 10 and 11', linux: 'Ubuntu 24.04 or later' },
    allVersions: 'All versions and release notes',
    footer: 'Records and transcribes calls on your computer. MIT · 2026', footLinks: 'Links', releases: 'Releases', policy: 'Privacy policy'
  }
};
export const CALLS: Record<Lang, string[]> = { ru: ['Google Meet', 'Zoom', 'Telegram', 'Teams', 'Slack', 'Discord', 'Телемост'], en: ['Google Meet', 'Zoom', 'Telegram', 'Teams', 'Slack', 'Discord', 'Telemost'] };
export const TECH: string[] = ['Rust', 'Tauri 2', 'Svelte 5', 'ONNX Runtime', 'llama.cpp', 'SQLite FTS5', 'sqlite-vec'];

export const LINES = LINES_RAW as Record<Lang, Line[]>;
export const T = T_RAW as unknown as Record<Lang, Dict>;
