# Google sign-in (OAuth) setup

YadaCall reads your upcoming calls from Google Calendar so it can remind you before a call and
record it. It asks for **read-only** access to calendar events and nothing else:

```
openid email profile https://www.googleapis.com/auth/calendar.events.readonly https://www.googleapis.com/auth/calendar.calendarlist.readonly
```

Tokens never leave your machine except to Google: the refresh token is stored in the OS keychain
(service `app.yadacall.desktop`, account `google:<email>`), the access token only in memory.
Signing out revokes the token at Google and deletes it from the keychain.

Google does not let an open-source app ship a shared secret safely, so every build needs an OAuth
client from a Google Cloud project. For a *Desktop app* client the "client secret" is not
confidential (see Google's installed-app guidance), so embedding it in the binary is fine.

## 1. Create the Google Cloud project

1. Open <https://console.cloud.google.com/> and create a project (for example `yadacall`).
2. **APIs & Services -> Library -> Google Calendar API -> Enable.**

## 2. Configure the consent screen

**APIs & Services -> OAuth consent screen** (in newer consoles: *Google Auth Platform*):

1. User type **External**.
2. App name `YadaCall`, support e-mail, developer contact e-mail.
3. **Scopes -> Add or remove scopes**: add
   `.../auth/userinfo.email`, `.../auth/userinfo.profile`, `openid` and
   `https://www.googleapis.com/auth/calendar.events.readonly` and
   `https://www.googleapis.com/auth/calendar.calendarlist.readonly` (the list of your calendars, so
   shared and team calendars appear too). Nothing else.
4. **Test users**: add the Google accounts you will sign in with (see "Publishing status").

## 3. Create the OAuth client

**APIs & Services -> Credentials -> Create credentials -> OAuth client ID**, application type
**Desktop app**. YadaCall uses the loopback redirect (`http://127.0.0.1:<random port>/callback`)
with PKCE, which Desktop clients allow without registering redirect URIs.

Copy the client ID (`...apps.googleusercontent.com`) and the client secret.

## 4. Give the client to the build

The names are the same everywhere:

| Variable | Meaning |
| --- | --- |
| `YADACALL_GOOGLE_CLIENT_ID` | OAuth client ID |
| `YADACALL_GOOGLE_CLIENT_SECRET` | OAuth client secret |

They are read at **compile time** (`option_env!`), so export them before building:

```sh
export YADACALL_GOOGLE_CLIENT_ID=1234-abc.apps.googleusercontent.com
export YADACALL_GOOGLE_CLIENT_SECRET=GOCSPX-...
cargo build --release
```

**Local development shortcut.** If the variables are not set, `crates/yc-connect/build.rs` reads
`$HOME/.config/yadacall/google-oauth.env` (`KEY=VALUE` lines, `#` comments allowed):

```sh
mkdir -p ~/.config/yadacall
cat > ~/.config/yadacall/google-oauth.env <<'ENV'
YADACALL_GOOGLE_CLIENT_ID=1234-abc.apps.googleusercontent.com
YADACALL_GOOGLE_CLIENT_SECRET=GOCSPX-...
ENV
chmod 600 ~/.config/yadacall/google-oauth.env
```

The build script re-runs when that file or the variables change, and never prints the values.

**CI / releases.** Store both as GitHub Actions secrets with the same names and pass them to the
build step:

```yaml
env:
  YADACALL_GOOGLE_CLIENT_ID: ${{ secrets.YADACALL_GOOGLE_CLIENT_ID }}
  YADACALL_GOOGLE_CLIENT_SECRET: ${{ secrets.YADACALL_GOOGLE_CLIENT_SECRET }}
```

**Runtime override.** The settings screen can override the compiled-in client (useful for people
who build from source with their own project). Precedence: settings override, then the runtime
environment, then the compiled-in values. With none of them set, "Add Google account" reports
"Google sign-in is not configured" and the app keeps working with ICS calendars.

## 5. Try it

```sh
cargo run -p yadacall-cli -- google login
cargo run -p yadacall-cli -- google accounts
cargo run -p yadacall-cli -- google agenda --days 3
cargo run -p yadacall-cli -- google logout you@example.com
```

## Publishing status: what to expect

`calendar.events.readonly` is a **sensitive scope**. Until the app is verified by Google:

- **Status "Testing"** (default): only the listed test users (max **100**) can sign in, and
  **refresh tokens expire after 7 days**. The account then shows "вход истёк · Войти снова" every
  week; signing in again fixes it. This is Google policy, not a bug.
- **Status "In production" but unverified**: no 7-day expiry and no test-user list, but users see
  the "Google hasn't verified this app" warning (Advanced -> Go to YadaCall) and the project is
  capped at **100 users** until verification.
- **Verified**: no warning and no cap. Verification needs a public privacy policy, a homepage on a
  domain you own, and a short screencast of the consent flow; Google reviews sensitive scopes
  without the paid security assessment (that is only for restricted scopes).
- **Google Workspace accounts**: administrators can block unverified third-party apps, so signing
  in with a work account may fail with "access blocked" until the admin allows the client ID (or
  the app is verified). Personal accounts are not affected.

If sign-in is a problem, subscribing to a calendar's secret ICS URL needs no OAuth at all
(`yadacall ics add <url>`; Google Calendar -> Settings -> *Integrate calendar* -> *Secret address
in iCal format*).
