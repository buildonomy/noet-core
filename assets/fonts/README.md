# Noet Fonts

This directory contains vendored font files for the Noet HTML viewer.

## Current Font: IBM Plex Sans

IBM Plex Sans is used as the primary reading font for Noet documents. It provides:
- Excellent readability at paragraph lengths
- Slight technical/geometric character for "working document" feel
- Modern, professional appearance
- Good distinction between similar characters (l, I, 1, etc.)

### Current Implementation: Vendored (Self-Hosted)

The fonts are vendored locally in this directory:
- **Location**: `assets/fonts/ibm-plex-sans/woff2/`
- **Weights**: 300, 400, 500, 600, 700 (regular and italic)
- **Format**: WOFF2 (modern browsers)
- **Total Size**: ~1.1MB for all weights
- **Import**: `@import url("fonts/ibm-plex-sans.css")` in theme files
- **Performance**: `font-display: swap` for graceful fallback

**Advantages**:
- No external dependencies (works offline)
- No privacy concerns (no CDN requests)
- Predictable performance
- Complete control over font files

**File Structure**:

```
assets/fonts/
├── ibm-plex-sans.css                    (font-face declarations)
└── ibm-plex-sans/
    ├── LICENSE.txt
    └── woff2/
        ├── IBMPlexSans-Light.woff2          (300)
        ├── IBMPlexSans-LightItalic.woff2    (300 italic)
        ├── IBMPlexSans-Regular.woff2        (400)
        ├── IBMPlexSans-Italic.woff2         (400 italic)
        ├── IBMPlexSans-Medium.woff2         (500)
        ├── IBMPlexSans-MediumItalic.woff2   (500 italic)
        ├── IBMPlexSans-SemiBold.woff2       (600)
        ├── IBMPlexSans-SemiBoldItalic.woff2 (600 italic)
        ├── IBMPlexSans-Bold.woff2           (700)
        └── IBMPlexSans-BoldItalic.woff2     (700 italic)
```

### Updating Fonts

To update to a newer version of IBM Plex Sans:

1. **Download from IBM GitHub**:
   ```bash
   cd assets/fonts/ibm-plex-sans
   curl -L -o web.zip https://github.com/IBM/plex/releases/download/v6.4.0/Web.zip
   unzip web.zip
   cp -r Web/IBM-Plex-Sans/fonts/complete/woff2 .
   cp Web/LICENSE.txt .
   rm -rf Web web.zip
   ```

2. **Verify files**: Check that all required weights are present
3. **Test**: Rebuild and test in browser

### Alternative: Google Fonts CDN

If you prefer using Google Fonts CDN instead of vendored fonts:

Replace `@import url("fonts/ibm-plex-sans.css")` in both theme files with:

```css
@import url("https://fonts.googleapis.com/css2?family=IBM+Plex+Sans:ital,wght@0,300;0,400;0,500;0,600;0,700;1,300;1,400;1,500;1,600;1,700&display=swap");
```

**CDN Advantages**:
- Smaller initial bundle (fonts loaded separately)
- Automatic optimization and subsetting
- Potential cross-site caching

**CDN Disadvantages**:
- Requires internet connection
- External dependency on Google infrastructure
- Privacy considerations (requests to Google servers)

### Font Subsetting (Optional)

The vendored fonts include full character sets. To reduce size further, create Latin-only subsets:

```bash
# Using pyftsubset (part of fonttools)
pip install fonttools brotli

# Latin + common punctuation + programming symbols
pyftsubset IBMPlexSans-Regular.woff2 \
    --output-file=IBMPlexSans-Regular-subset.woff2 \
    --flavor=woff2 \
    --layout-features='*' \
    --unicodes="U+0020-007E,U+00A0-00FF,U+2010-2027,U+2030-205E"
```

**Expected size reduction**: ~50-70% for Latin-only subset (brings total from 1.1MB to ~300-500KB)

### Font Weight Usage

The following CSS custom properties define the font weight hierarchy:

- `--noet-font-weight-ui: 350` - Lighter weight for navigation, metadata, footer
- `--noet-font-weight-body: 400` - Standard reading weight for body content
- `--noet-font-weight-medium: 500` - Medium weight (future use)
- `--noet-font-weight-heading: 600` - Headings in content
- `--noet-font-weight-bold: 700` - Strong emphasis

### Performance Considerations

**Vendored (current)**:
- Total size: ~1.1MB for all weights (full character set)
- Per weight: ~60-70KB
- Caching: Browser cache only
- Blocking: None (`font-display: swap`)
- Works offline: Yes

**Google Fonts CDN** (alternative):
- Initial load: ~20-40KB (compressed, auto-subset)
- Caching: Browser cache + CDN cache
- Blocking: None (`font-display: swap`)
- Works offline: No

### License

IBM Plex is licensed under the SIL Open Font License 1.1:
- **Commercial use**: Allowed
- **Modification**: Allowed
- **Distribution**: Allowed
- **Requirements**: Include copyright notice and license

License: https://github.com/IBM/plex/blob/master/LICENSE.txt

## Alternative Fonts

If IBM Plex Sans doesn't meet your needs, consider:

### Inter
- Modern, versatile sans-serif
- Optimized for UI and reading
- Similar aesthetic to IBM Plex Sans
- https://rsms.me/inter/

### JetBrains Mono (Sans variant)
- More technical/monospace character
- Excellent for code-heavy documents
- Slightly more condensed
- https://www.jetbrains.com/lp/mono/

### System Fonts (Fallback)
The font stack includes system fallbacks:
```css
"IBM Plex Sans", -apple-system, BlinkMacSystemFont, "Segoe UI", system-ui, sans-serif
```

This ensures readable text even if IBM Plex Sans fails to load.

## Testing Font Changes

After changing fonts:

1. **Visual testing**: Check rendering in Chrome, Firefox, Safari
2. **Weight testing**: Verify hierarchy (UI lighter than content, headings bold)
3. **Theme testing**: Test both light and dark themes
4. **Performance testing**: Measure load time impact (<100ms target)
5. **Accessibility testing**: Verify WCAG AA contrast ratios maintained

## Related Issue

See `docs/project/ISSUE_44_UI_CLEANUP.md` Phase 5 for implementation context.