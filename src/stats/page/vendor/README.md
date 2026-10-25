# Vendored chart libraries

The stats page (`grans stats --html`) is one self-contained file that makes no
network requests, so the libraries it draws with are compiled into grans and
inlined into the page.

| File | Package | Version | From |
|---|---|---|---|
| `d3.min.js` | `d3` | 7.9.0 | `dist/d3.min.js` in the npm tarball |
| `plot.umd.min.js` | `@observablehq/plot` | 0.6.17 | `dist/plot.umd.min.js` in the npm tarball |

Both are ISC licensed; the licenses are alongside. Plot's UMD build reads d3
from the global `d3`, so `d3.min.js` must load first.

To update one, download the tarball from the npm registry, check it against
the registry's `dist.shasum`, and copy the file over unchanged.
