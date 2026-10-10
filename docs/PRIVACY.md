# Privacy policy

MISO Terminal has no telemetry, analytics, crash reports or accounts with
the project. It connects only to the sources whose data it shows and to
services you set it up to use, and it sends them only what a request for that
data needs. Nothing it stores leaves your PC except as described here.

## What it connects to

Every connection is HTTPS (WebSocket streams are WSS). Each server sees your
IP address and the program's name, version and repository in the
`User-Agent` header (`MISO-Terminal/<version>
(+https://github.com/ArenKDesai/miso-terminal)`), as with any web request.
The terminal keeps no cookies.

- **MISO** (`public-api.misoenergy.org`, `docs.misoenergy.org`), for panels
  with MISO data, the price history's backfill and the forecasts kept as
  issued (its daily load forecast reports, wind and solar forecasts and
  outage schedule, asked for in the background while the terminal runs
  unless *Forecasts kept as issued* is off in `SET`): requests for public
  data.
  [MISO's policy](https://www.misoenergy.org/meet-miso/legal-and-privacy/).
- **The National Weather Service** (`api.weather.gov`), for `WX` and the
  forecasts kept as issued (every three hours in the background, unless
  turned off in `SET`): forecasts for fixed cities across MISO, never your
  location.
  [NWS's policy](https://www.weather.gov/privacy).
- **The EIA** (`www.eia.gov`), for `GAS` and HOME: the Henry Hub price file.
  [EIA's policy](https://www.eia.gov/about/privacy_security_policy.php).
- **News feeds,** for `TOP`, `NEWS`, `NI` and HOME: the publishers' public
  RSS feeds, from the
  [Financial Times](https://help.ft.com/legal-privacy/privacy-policy/),
  [Bloomberg](https://www.bloomberg.com/notices/privacy/) and
  [the Washington Post](https://www.washingtonpost.com/privacy-policy/), and
  any feeds you add under `[news]`.
- **Alpaca** (`data.alpaca.markets`, `stream.data.alpaca.markets`,
  `paper-api.alpaca.markets`), only once you enter your own Alpaca keys in
  `SET`: your keys (to Alpaca only), the symbols you look at, and the orders
  you confirm on a ticket. [Alpaca's policy](https://alpaca.markets/privacy-policy).
- **GitHub Pages** (`arenkdesai.github.io`), when `THEME` shows the theme
  gallery: the gallery's list and the theme you install.
  [GitHub's policy](https://docs.github.com/en/site-policy/privacy-policies/github-general-privacy-statement).

Opening a headline or a link hands it to your web browser; the terminal does
not fetch the article. A change that adds a service, or sends something new,
updates this page in the same pull request, and the
[changelog](../CHANGELOG.md) says so.

## What it keeps on your PC

- Settings, themes and fonts: `%APPDATA%\MISO Terminal\config`.
- The cache, the price history, the forecasts kept as issued, saved
  headlines (titles and links), the layout, logs, and the audit log of orders
  you placed:
  `%LOCALAPPDATA%\MISO Terminal`.
- API keys: Windows Credential Manager, never in files or logs. `SET`
  removes them.
- In portable mode, all of it except the keys sits in the `data` folder
  beside the program.

`LOG` shows these paths. `install.ps1 -Uninstall` removes the program and
keeps your data; delete the two folders (and the keys, from `SET` first) to
remove everything.

## Contact

Questions about this policy go in an
[issue](https://github.com/ArenKDesai/miso-terminal/issues).
