# Third-party notices

Trovara includes or is derived from the following works. Their licenses apply to those parts.

## Reference macOS application (behaviour and ported logic)

Parts of Trovara's engine logic (indexing rules, ranking, text normalization, prompts, index layout) are ported to
Rust from an MIT-licensed macOS application. Its notice:

```
MIT License

Copyright (c) 2026 Abdur Rahim

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

The files in `evals/`, `tests/fixtures/reference/` and `third_party/patches/llama.cpp-no-logits.patch` come from the
same work, under the same license.

## Shipped components

| Component | License |
|---|---|
| EmbeddingGemma 2 (Google DeepMind), downloaded at first run | Apache 2.0 |
| llama.cpp / ggml | MIT |
| PDFium (via bblanchon/pdfium-binaries) | BSD-3-Clause / Apache 2.0 |
| SQLite | Public domain |
| Rust crates | See `cargo about` output, generated into Acknowledgements.txt at release |
