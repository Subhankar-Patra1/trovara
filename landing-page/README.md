# Trovara landing page

The static website for the Trovara Windows application.
The site uses HTML, CSS, and JavaScript modules. It needs no build step.
It lives in the Trovara repository; the app's source is in the folders beside it.

## Preview locally

From the Trovara repository root, run:

```powershell
python -m http.server 47835 --directory landing-page/dist
```

Open <http://127.0.0.1:47835>.

Serve `dist/` with any static web host to use the page elsewhere.
Opening `index.html` directly through a `file:` URL does not load JavaScript modules in some browsers.

## Components

| File | Purpose |
|---|---|
| `dist/index.html` | Document metadata and theme initialization |
| `dist/components.js` | Shared navigation, search demo, icons, media previews, and dialogs |
| `dist/components-v2.js` | Navigation, format reference, setup illustrations, code search, comparison, video chapters, and FAQ |
| `dist/content.js` | Product links, sample files, queries, supported formats, FAQ copy |
| `dist/app.js` | Theme state, sample search, media controls, keyboard navigation, dialogs, release lookup |
| `dist/styles.css` | Shared design tokens, component styles, light and dark themes, responsive layouts |
| `dist/design.css` | Current page layout, typography, mobile layouts, and interaction states |
| `dist/refinements.css` | Rounded navigation, format reference, comparison panel, and button surfaces |
| `dist/assets/` | Self-hosted fonts, approved logo, demo images, video, audio, captions |
| `dist/credits.html` | Media sources and license information |
| `dist/updates.css` | Code search, Windows comparison, FAQ icons, update notes, hero font, and the 12 px phone text floor |

## Behavior

- The theme control offers System, Light, and Dark. It stores the choice locally.
- System mode follows changes to the operating-system theme.
- Product navigation opens a menu with section links. Smaller screens use an expandable menu.
- Sample search matches six supplied fictional examples. It does not read visitor files.
- File-type tabs work with pointer input and arrow keys.
- Sample buttons open the matching example in the main search demo.
- Setup steps support arrow keys. Sample folder selections persist between steps.
- One product-video player opens from the hero. Chapter buttons sit inside its dialog.
- FAQ filters show setup or file questions.
- System requirements expand from the download panel.
- A table compares Windows search with Trovara. On phones each row becomes a card.
- The code search section shows an example `code:` query and its matching function.
- The download panel explains the SmartScreen prompt and shows the latest release from GitHub.
- Buttons use flat rounded surfaces, thin neutral borders, and visible keyboard focus.
- The local search diagram animates once after a button press. Reduced-motion mode shows a text description.
- Native dialogs provide preview and video controls. Escape closes a dialog and restores focus.
- The video includes English captions. Playback starts only after a user action.
- Reduced-motion settings disable the search-icon animation and query-typing effect.
- GitHub’s public release API supplies a published Windows installer link when one exists.
- Without a published installer, the download panel links to release status.
- The site uses no analytics, tracking scripts, or third-party font requests.
- GitHub release lookup makes a network request. Product source links lead to external websites.

## Content and downloads

Each section has a distinct purpose. The hero contains the only live file demonstration.
File support lists formats and reading limits. Setup explains the installation steps.
Local processing explains data handling and the model. Comparison aligns tool differences.
The FAQ covers five remaining questions. Download contains the release link and requirements.

Change `config` in `dist/content.js` if the repository or releases address changes.
The release lookup accepts a setup executable from the configured repository only.
It supports public prereleases.

The installation requirements follow the project README.
Check these requirements when the application release changes.
Update the absolute Open Graph URLs in `dist/index.html` if you use a custom domain.

## Media

The page reuses the approved Trovara logo and the original red-car image from the product video.
It includes the 75-second narrated product video.
The lake clip, music, and font retain their source licenses.
See `dist/credits.html` before publishing the video elsewhere.
