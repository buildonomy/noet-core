# Third-Party Licenses

This file documents vendored third-party assets included in the noet-core binary.
Source files live under `assets/` and are embedded at compile time via `include_dir!`.

---

## Tabulator v6.4.0

**Package**: `tabulator-tables`  
**Version**: `6.4.0`  
**License**: MIT  
**Source**: https://github.com/olifolkerd/tabulator  
**npm**: https://www.npmjs.com/package/tabulator-tables  
**Vendored files**:
- `assets/tabulator/tabulator.min.css`
- `assets/tabulator/tabulator.min.js`

**Usage**: Client-side sortable, filterable, paginated HTML tables for XLSX/ODS tab
rendering in the noet HTML viewer. Initialized by `XlsxCodec::generate_html()`.

**Upgrade path**: Run `npm install && npm run copy:tabulator` from the `assets/`
directory after updating the version in `assets/package.json`.

```
MIT License

Copyright (c) 2015-2024 Oli Folkerd

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
```

---

## KaTeX v0.16.x

**Package**: `katex`  
**License**: MIT  
**Source**: https://github.com/KaTeX/KaTeX  
**npm**: https://www.npmjs.com/package/katex  
**Vendored files**:
- `assets/katex/katex.min.css`
- `assets/katex/katex.min.js`
- `assets/katex/auto-render.min.js`
- `assets/katex/fonts/*`

**Usage**: Math formula rendering in Markdown documents.

**Upgrade path**: Run `npm install && npm run copy:katex` from the `assets/`
directory after updating the version in `assets/package.json`.

```
MIT License

Copyright (c) 2013-2020 Khan Academy and other contributors

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
```

---

## Open Props v1.7.x

**Package**: `open-props`  
**License**: MIT  
**Source**: https://github.com/argyleink/open-props  
**npm**: https://www.npmjs.com/package/open-props  
**Vendored files**:
- `assets/open-props/open-props.min.css`
- `assets/open-props/normalize.min.css`

**Usage**: CSS design tokens (color, spacing, typography) for the HTML viewer.
Can be replaced with CDN versions via the `--cdn` CLI flag.

**Upgrade path**: Run `npm install && npm run copy:open-props` from the `assets/`
directory after updating the version in `assets/package.json`.

```
MIT License

Copyright (c) 2021 Adam Argyle

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
```
