# Published CDDL corpus

Schemas written by others, which `tests/corpus.rs` runs through cddlc
(dgtlrift/cddlc#113). They are copied unchanged.

| Directory | Source | License |
|---|---|---|
| `rfc/` | The CDDL extracted from RFCs, as bundled by [cabo/cddlc](https://github.com/cabo/cddlc) in `data/` (commit `261bea5`, 2025-02-16) | IETF Code Components: Revised BSD (see each RFC); the collection is MIT, `rfc/LICENSE` |
| `webdriver-bidi/` | [WebDriver BiDi](https://w3c.github.io/webdriver-bidi/), as bundled by [google/cddlconv](https://github.com/google/cddlconv) in `examples/webdriver-bidi/` (commit `110b5b4`, 2026-11-11) | Apache-2.0, `webdriver-bidi/LICENSE` |

Some files are fragments that use another RFC's rules; the test lists those with the
files they need. A schema cddlc can't take yet is listed with the error it gives and the
issue that tracks it.
