# @nullradix/theme-sdk

The theme SDK describes the contract between the NULLRADIX interface (the host) and a theme: which
places a theme may replace (slots), which data and parts it gets from the host (hooks and parts),
and what the manifest looks like.

Current version: `SDK_VERSION` = **1.3.0** (equal to the package version in `package.json` and to
`SDK_VERSION` in `crates/domain/src/theme.rs`; `cargo xtask check` fails when they differ).

> This README is the authoritative SDK documentation. `tests/readme.test.ts` checks that every slot
> and every export is named here.

## Overview

- **Plain CSS themes** ship only `theme.css` and set CSS variables.
- **Themes with code** also ship `main.js` (ESM) and replace slots with their own React components.
  Theme code runs in the app's own page without a sandbox, so it only runs after an explicit
  per-theme switch, and themes install only from repos the user added, with a sha256 check.
- Only a slot can be replaced. A missing slot keeps the default component.
- Light and dark are built in; every other theme is a package.

### Package format `.nullradix`

A theme is a ZIP with the extension `.nullradix`:

| File            | Required | Content                                                                                                                           |
| --------------- | -------- | --------------------------------------------------------------------------------------------------------------------------------- |
| `manifest.json` | yes      | Description of the theme (see [Manifest](#manifest))                                                                              |
| `theme.css`     | yes      | CSS variable values and own rules                                                                                                 |
| `main.js`       | no       | ESM module with the `ThemeDefinition`                                                                                             |
| `assets/**`     | no       | Images, fonts and so on                                                                                                           |
| `claude/**`     | no       | Claude theme for Claude Desktop (since 1.3.0, see [Claude theme](#claude-theme)); only with `"claudeTheme": true` in the manifest |

Installed themes live under `~/.nullradix/themes/<id>/`.

### Claude theme

A theme can ship a matching look for Claude Desktop. It is a Claude Code mod under `claude/hooks/`
in the package: `hooks.json` (`{ "modules": ["./register.tsx"] }`, required), `register.tsx` and
more `.ts` or `.tsx` files. TypeScript is allowed only below `claude/`. NULLRADIX rejects a package
with `claude/` but without `"claudeTheme": true`, and the other way round without
`claude/hooks/hooks.json`.

## Manifest

```json
{
  "id": "night",
  "name": "Night",
  "version": "1.2.3",
  "author": "Ada",
  "sdkVersion": "^1.0.0",
  "colorScheme": "dark",
  "startColor": "#0b0b10",
  "preview": "assets/preview.png",
  "slots": ["Shell", "Progress"],
  "permissions": { "network": ["fonts.example.com"] },
  "settingsSchema": {
    "stars": { "type": "boolean", "label": "Starry sky", "default": true }
  }
}
```

| Field                 | Required | Meaning                                                                                                  |
| --------------------- | -------- | -------------------------------------------------------------------------------------------------------- |
| `id`                  | yes      | Identifier, pattern `^[a-z0-9-]{2,40}$`; also the value of `data-theme`                                  |
| `name`                | yes      | Display name                                                                                             |
| `version`             | yes      | Version of the theme (semver)                                                                            |
| `author`              | yes      | Author                                                                                                   |
| `sdkVersion`          | yes      | Semver range of matching SDK versions, such as `^1.0.0`                                                  |
| `colorScheme`         | yes      | `light` or `dark`: the base scheme the theme builds on                                                   |
| `startColor`          | no       | Background color at start, before `theme.css` has loaded                                                 |
| `preview`             | no       | Path to a preview image in the package                                                                   |
| `slots`               | no       | List of the slots `main.js` replaces                                                                     |
| `permissions.network` | no       | Domains the theme may load through the server proxy                                                      |
| `settingsSchema`      | no       | Own settings (see [Theme settings](#theme-settings))                                                     |
| `claudeTheme`         | no       | `true` when the package ships a Claude theme under `claude/` (since 1.3.0); needs `"sdkVersion": "^1.3"` |

## ThemeDefinition

`main.js` describes the theme as a `ThemeDefinition`:

| Field            | Type                                        | Meaning                                                                                                                        |
| ---------------- | ------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------ |
| `id`             | `string`                                    | Id of the theme, equal to `id` in the manifest                                                                                 |
| `slots`          | `ThemeSlots`                                | Replaced slots                                                                                                                 |
| `settingsSchema` | `ThemeSettingsSchema` (optional)            | Own settings                                                                                                                   |
| `mermaid`        | `() => MermaidLook` (optional, since 1.2.0) | Look of Mermaid diagrams; called on every draw, so CSS variables can be read with `getComputedStyle(document.documentElement)` |

```tsx
import type { ThemeDefinition, ProgressProps } from '@nullradix/theme-sdk';

function Progress({ done, total, label }: ProgressProps) {
  return <progress aria-label={label} value={done} max={total} />;
}

const theme: ThemeDefinition = { id: 'night', slots: { Progress } };
export default theme;
```

## Slots

`ThemeSlots` lists every place a theme may replace. Every slot is optional.

| Slot             | Props                   | Replaces                                                                              |
| ---------------- | ----------------------- | ------------------------------------------------------------------------------------- |
| `Shell`          | `ShellProps`            | the whole layout around the pages                                                     |
| `NavIcon`        | `{ name: NavIconName }` | icons of the main navigation                                                          |
| `Icon`           | `{ name: IconName }`    | other core icons: docs tree, empty state, buttons on the text selection (since 1.2.0) |
| `Progress`       | `ProgressProps`         | progress display (done and all steps)                                                 |
| `Button`         | `ButtonProps`           | buttons                                                                               |
| `Card`           | `CardProps`             | inside of a card                                                                      |
| `Panel`          | `PanelProps`            | inside of an area (such as settings)                                                  |
| `Dialog`         | `DialogProps`           | visible shell of a dialog                                                             |
| `Input`          | `InputProps`            | input fields (single and multi line)                                                  |
| `StartAnimation` | `StartAnimationProps`   | loading screen at start                                                               |
| `page:dashboard` | `PageProps`             | the whole dashboard page                                                              |

**Errors and safe mode:** every slot sits in an error boundary. When a theme component throws, the
host shows the default component and a notice. The address parameter `?safe-theme` starts the
interface without any theme code.

### `ShellProps`

| Field     | Type         | Meaning                                                      |
| --------- | ------------ | ------------------------------------------------------------ |
| `pid`     | `string`     | Project id from the address                                  |
| `nav`     | `ReactNode`  | Links of the main navigation                                 |
| `content` | `ReactNode`  | Content area with the connection notice and the current page |
| `parts`   | `ShellParts` | More parts of the frame                                      |

`ShellParts` (also through `useShellParts()`):

| Field           | Type                                      | Meaning                                   |
| --------------- | ----------------------------------------- | ----------------------------------------- |
| `search`        | `ReactNode`                               | Button of the global search (also Ctrl+K) |
| `boardSwitcher` | `ReactNode`                               | Board switcher with the connection dot    |
| `usage`         | `ReactNode`                               | Claude usage (ring or bar)                |
| `navItems`      | `Partial<Record<NavIconName, ReactNode>>` | Single links of the main navigation       |
| `outlet`        | `ReactNode`                               | Only the current page                     |
| `connection`    | `ReactNode`                               | Banner while the live connection is down  |
| `status`        | `ReactNode`                               | Dot of the live connection                |

`NavIconName`: `board`, `dashboard`, `inbox`, `sessions`, `questions`, `docs`, `search`, `guide`, `settings`.

`IconName`: `doc`, `plan`, `glossary`, `adr`, `file` (files in the docs tree), `folder`, `folderOpen`
(folders in the docs tree; without the slot the core shows no icon there), `empty` (empty state),
`edit`, `comment` (buttons on the text selection in docs).

`MermaidLook`: `theme` (`base`, `default`, `dark`, `neutral`, `forest`), `themeVariables`, `themeCSS`,
`fontFamily`, as in the Mermaid config. Without `mermaid` the core keeps its default look.

### `ProgressProps`

`done: number`, `total: number`, `label: string` (accessible name), `caption: string` (caption next
to the number).

### `ButtonProps`

`variant?: 'primary' | 'secondary' | 'danger' | 'ghost'` (default `secondary`),
`type?: 'button' | 'submit'`, `disabled?`, `onClick?`, `title?`, `autoFocus?`, `aria-label?`,
`aria-pressed?`, `aria-expanded?`, `className?`, `children?`. The theme renders the `<button>`
itself and passes the props on.

### `CardProps` and `PanelProps`

`CardProps`: `title?`, `actions?`, `className?`, `children?`. `PanelProps`: `title?`, `className?`,
`children?`. The outer element stays in the host and carries `data-slot="Card"` or `data-slot="Panel"`.

### `DialogProps`

`labelledBy: string` (id of the heading), `role: 'dialog' | 'alertdialog'`, `className?`,
`children?`. Role, focus trap, Esc and focus return stay in the host (`data-slot="Dialog"`).

### `InputProps`

`value: string`, `onChange(value: string)`, `id?`, `name?`, `type?` (`text`, `search`, `password`,
`email`, `url`, `number`), `placeholder?`, `disabled?`, `required?`, `autoFocus?`, `multiline?`,
`rows?`, `aria-label?`, `aria-describedby?`, `className?`. The theme renders `<input>` or `<textarea>`.

### `StartAnimationProps`

`onDone(): void` - must be called when the animation is over. It covers the app and ends after 8 s
at the latest.

### Page slots: `PageName`, `PageProps`, `PageSlots`

`PageSlots` are keys of the form `page:<name>` with `<name>` from `PageName` (currently only
`dashboard`). `PageProps` holds `pid: string`; the theme page loads its data through the hooks.

## Hooks

All hooks work only inside the NULLRADIX interface. Outside the `ThemeHostProvider` they throw an
error naming the hook and the provider. Only the host renders `ThemeHostProvider`; `ThemeHost` is
the type of what it provides.

| Hook                        | Returns                                                   | Meaning                                                                                               |
| --------------------------- | --------------------------------------------------------- | ----------------------------------------------------------------------------------------------------- |
| `useNav()`                  | `NavItem[]`                                               | Entries of the main navigation                                                                        |
| `useBoards()`               | `BoardsState`                                             | Projects, boards and the board choice                                                                 |
| `useUsage()`                | `UsageState`                                              | Claude usage                                                                                          |
| `useSettings()`             | `SettingsState`                                           | Read and change the user settings                                                                     |
| `useThemeSettings<S>()`     | `ThemeSettingsState` with `values: ThemeSettingValues<S>` | Own settings of the theme                                                                             |
| `useColorScheme()`          | `ColorSchemeState`                                        | Shown color scheme                                                                                    |
| `useNavigate()`             | `(to: string) => void`                                    | Change the route                                                                                      |
| `useOpenSearch()`           | `() => void`                                              | Open the global search                                                                                |
| `useShellParts()`           | `ShellParts`                                              | Ready-made parts of the frame                                                                         |
| `useThemeAsset(url, opts?)` | `ThemeAssetState`                                         | Foreign resource through the server proxy (see [Foreign resources](#foreign-resources-usethemeasset)) |

Types:

- `NavItem`: `id: NavIconName`, `label`, `href`, `active`, `badge?`. `badge` is the number of open
  items of the page; the host fills it for `inbox` (my tasks), `questions` (own open questions) and
  `docs` (new docs). It is missing at 0 and while the number is not known yet.
- `BoardsState`: `current: BoardRef | null`, `projects`, `boards`, `loading`, `choose(ref: BoardRef)`.
  `BoardRef`: `project: string`, `board: number | null`.
- `UsageState`: `enabled`, `available`, `reason`, `fiveHour`, `sevenDay` (each `UsageWindow | null`).
  `UsageWindow`: `percent`, `resetsAt` (ISO instant or null).
- `SettingsState`: `values: ThemeSettings | null`, `loading`, `update(patch)`.
  `ThemeSettings`: `theme`, `claudeUsage`, `compactCards`, `heroImages`.
- `ColorSchemeState`: `scheme: 'light' | 'dark'`, `themeId`, `mode`.

```tsx
import { useBoards, useNavigate } from '@nullradix/theme-sdk';

function Boards() {
  const { boards } = useBoards();
  const navigate = useNavigate();
  return boards.map((b) => (
    <button key={b.id} onClick={() => navigate(`/p/${b.project}/board`)}>
      {b.name}
    </button>
  ));
}
```

## Foreign resources: `useThemeAsset`

A theme never loads images, fonts or files from a JAR or ZIP straight from a foreign server (CORS,
privacy) but through the NULLRADIX server proxy. `useThemeAsset(url, opts?)` builds the local proxy
URL `/api/themes/<id>/fetch?url=<encoded>` (plus `sha256` and `entry` when set), checks it with
`fetch` and reports the loading and error state. A theme may only load domains listed in
`permissions.network` of the manifest; otherwise the proxy answers 403. Resources are cached per theme.

```ts
function useThemeAsset(url: string | null | undefined, opts?: ThemeAssetOptions): ThemeAssetState;
function themeAssetUrl(
  themeId: string,
  url: string,
  opts?: Omit<ThemeAssetOptions, 'themeId'>,
): string;
```

`ThemeAssetOptions`:

| Field      | Meaning                                                                                                 |
| ---------- | ------------------------------------------------------------------------------------------------------- |
| `themeId?` | Theme whose grant and cache apply; default is the active theme of the host (`useColorScheme().themeId`) |
| `sha256?`  | Expected checksum (hex); on a mismatch the proxy answers 502 `theme_fetch_checksum`                     |
| `entry?`   | Path of an entry when `url` is a ZIP or JAR, such as `assets/minecraft/textures/block/dirt.png`         |

`ThemeAssetState`:

| `status`  | `url`     | `error`           | Meaning                                                      |
| --------- | --------- | ----------------- | ------------------------------------------------------------ |
| `idle`    | `null`    | `null`            | No URL given (`null`, `undefined`, empty), nothing is loaded |
| `loading` | `null`    | `null`            | Request in flight                                            |
| `ready`   | proxy URL | `null`            | Resource reachable; use the URL in `<img src>` or `url(...)` |
| `error`   | `null`    | `ThemeAssetError` | Loading failed                                               |

When `url` or an option changes, the hook reloads and aborts the old request; it does so on unmount
too. The hook keeps no bytes; the browser loads the image from its HTTP cache.

In the browser the proxy URL is a path (`/api/themes/<id>/fetch?...`). In the desktop app it points
at the local server and carries its start token (`http://127.0.0.1:<port>/api/themes/<id>/fetch?...&token=...`).
Port and token change with every start of the app, so do not store the URL; build it again when needed.

`ThemeAssetError`: `status: number | null` (HTTP status, `null` on a network error), `code: string`,
`message: string`. The `code` comes from the JSON reply of the proxy:

| `code`                        | Status    | Cause                                               |
| ----------------------------- | --------- | --------------------------------------------------- |
| `theme_fetch_forbidden`       | 403       | Domain is not in `permissions.network` of the theme |
| `theme_fetch_too_large`       | 413       | Resource is too large                               |
| `theme_fetch_checksum`        | 502       | `sha256` does not match the content                 |
| `theme_fetch_upstream`        | 502       | Foreign server unreachable or answers with an error |
| `theme_fetch_entry_not_found` | 404       | `entry` is missing in the archive                   |
| `theme_fetch_not_archive`     | 502       | `entry` set, but the foreign server delivers no ZIP |
| `bad-request`                 | 400       | Invalid URL or parameter                            |
| `network`                     | -         | Network error while calling the proxy               |
| `http_<status>`               | per reply | Reply without a JSON error, such as `http_404`      |

Without `ThemeHostProvider` and without `opts.themeId` the hook throws.

```tsx
import { useThemeAsset } from '@nullradix/theme-sdk';

// Texture from a client JAR; the domain must be in permissions.network
function Dirt() {
  const asset = useThemeAsset('https://piston-data.mojang.com/v1/objects/abc/client.jar', {
    sha256: '3f2a...',
    entry: 'assets/minecraft/textures/block/dirt.png',
  });
  if (asset.status === 'loading') return <span>loading ...</span>;
  if (asset.status === 'error') return <span>Image unavailable ({asset.error?.code})</span>;
  return asset.url ? <img src={asset.url} alt="Dirt" /> : null;
}
```

For CSS without state, `themeAssetUrl` returns only the proxy URL and checks nothing:

```tsx
import { themeAssetUrl, useColorScheme } from '@nullradix/theme-sdk';

function Background() {
  const { themeId } = useColorScheme();
  const image = themeAssetUrl(themeId, 'https://fonts.example.com/pattern.png');
  return <div style={{ backgroundImage: `url(${image})` }} />;
}
```

## Parts

Ready-made host components for custom shells:

| Part               | Meaning                                                                                                                                       |
| ------------------ | --------------------------------------------------------------------------------------------------------------------------------------------- |
| `NavLink`          | Link of a navigation entry: `id`, `className?`, `role?`, `onClick?`, `children?: (item: NavItem) => ReactNode`; an unknown id renders nothing |
| `BoardSwitcher`    | Board switcher                                                                                                                                |
| `SearchButton`     | Button of the global search                                                                                                                   |
| `UsageMeter`       | Claude usage                                                                                                                                  |
| `Outlet`           | Current page without the default wrapper                                                                                                      |
| `ConnectionStatus` | Dot of the live connection                                                                                                                    |

```tsx
import { NavLink, Outlet, SearchButton } from '@nullradix/theme-sdk';
import type { ShellProps } from '@nullradix/theme-sdk';

function Shell(_: ShellProps) {
  return (
    <div className="my-shell">
      <header>
        <NavLink id="board" />
        <NavLink id="inbox" />
        <SearchButton />
      </header>
      <main>
        <Outlet />
      </main>
    </div>
  );
}
```

## Theme settings

A theme can ship its own settings (`settingsSchema` in the manifest and in `ThemeDefinition`). They
show under Settings, Appearance.

- `ThemeSettingsSchema`: `Record<string, ThemeSetting>`, keys `^[a-zA-Z][a-zA-Z0-9_-]{0,39}$`, at most 50.
- `ThemeSetting` = `ThemeSettingBoolean` (`type: 'boolean'`, `label`, `hint?`, `default: boolean`)
  or `ThemeSettingSelect` (`type: 'select'`, `label`, `hint?`, `options: { value, label }[]`, `default: string`).
- `ThemeSettingValues<S>`: values for the schema, switches as `boolean`, choices as `string`.
- `ThemeSettingsState`: `values`, `loading`, `set(key, value)`; missing or invalid values give the default.

```tsx
const schema = { stars: { type: 'boolean', label: 'Starry sky', default: true } } as const;
const { values, set } = useThemeSettings<typeof schema>();
```

## CSS variables

A theme sets values in `theme.css` under the selector `:root[data-theme='<id>']`. User CSS snippets
lie above the active theme. A theme loads foreign resources (fonts, images) only through the server
proxy and only from domains in `permissions.network`.

```css
:root[data-theme='night'] {
  color-scheme: dark;
  --bg: #0b0b10;
  --accent: #8b5cf6;
}
```

The list of variables belongs to the web app and is not part of this package.

## Versioning

The SDK follows semver since 1.0.0. `SDK_VERSION` always equals the version in `package.json` and
the one in `crates/domain/src/theme.rs`.

- **Major:** a break in slot, props, hook or part types (field removed or renamed, type narrowed,
  slot removed) or in CSS variables.
- **Minor:** new slots, hooks, parts, optional fields or CSS variables.
- **Patch:** docs and fixes without a change of the types.

Changes:

| Version | Change                                                                                                        |
| ------- | ------------------------------------------------------------------------------------------------------------- |
| 1.3.0   | new: manifest field `claudeTheme` and folder `claude/**` in the package (Claude theme for Claude Desktop)     |
| 1.2.0   | new: slot `Icon` with `IconName`, field `mermaid` of `ThemeDefinition` with `MermaidLook`                     |
| 1.1.0   | new: `useThemeAsset`, `themeAssetUrl` and the types `ThemeAssetOptions`, `ThemeAssetState`, `ThemeAssetError` |
| 1.0.0   | first stable API                                                                                              |

The host loads only themes whose `sdkVersion` range contains the current `SDK_VERSION`; others are
not loaded and are left out of the store as not matching. Recommended: `"sdkVersion": "^1.0.0"`.

## Build

`npm run build -w sdk/theme` compiles `src` with `tsc` into `dist` with `.d.ts` files. The package
is private for now; publishing comes later.
