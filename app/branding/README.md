# Trovara logo

The user supplied and selected `Gemini_Generated_Image_wrhn34wrhn34wrhn.png`.
The mark combines a tall white T-shaped folder, four file cards, a glass search
lens, and an orange handle on a charcoal tile.
The cards represent documents, video, images, and audio.

`icon-source.png` is the source for all icon sizes.
The source is 2048 by 2048 pixels.
The built-in imagegen tool removed the small Gemini mark from the lower-right
background. The edit prompt requested only that removal and required the logo,
colors, lighting, border, and framing to remain unchanged.
Only the repaired 128 by 128 pixel region at (1744, 1744) was transferred to the
original image. The patch color matches the surrounding background.
Its boundary uses a 12-pixel blend.
A pixel comparison confirmed zero color changes outside that region.
The original image in Downloads was not modified.

The exterior black background is now transparent.
The rounded charcoal tile, its gray border, and all artwork remain intact.
The built-in imagegen tool supplied an exterior transparency mask.
Only that mask's alpha channel was applied to the original image.
A pixel comparison confirmed zero changes to the image's RGB color channels.

Run this command from the repository root to regenerate the assets:

```powershell
powershell -File scripts/generate-icons.ps1
```

The script uses the Tauri CLI installed in `app/node_modules`.
It does not build the app.
Intermediate files go to the ignored `target/brand-icons` directory.

| File | Use |
|---|---|
| `app/src-tauri/icons/icon.ico` | Windows app and installer |
| `app/src-tauri/icons/icon.png` | Main PNG icon |
| `app/src-tauri/icons/32x32.png` | Small Windows icon |
| `app/src-tauri/icons/128x128.png` | Medium Windows icon |
| `app/src-tauri/icons/128x128@2x.png` | Large Windows icon |
| `app/src-tauri/icons/tray.png` | System tray |
| `app/ui/icon.png` | Setup and Settings |
| `app/ui/logo.png` | Large logo on Settings → About |
