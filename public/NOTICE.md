# Source and third-party notices

This file records provenance found in the source tree. It does not grant a
license, assert ownership of imported code, or replace any component's license.

- SessionDock's production frontend derives from an earlier Python frontend in
  the private development project. The historical frozen reference and its
  images are not included in this snapshot.
- `crates/ptyhost` was imported as an independent session host. The repository
  maintainer must establish the applicable redistribution rights for this source
  before publishing; this notice does not invent an upstream license.
- `web/vendor/highlight/` includes its BSD license in `LICENSE`.
- `web/vendor/katex/` includes its MIT license in `LICENSE` and bundled font assets.
- `web/fonts/` includes Ubuntu Font Licence 1.0, SIL Open Font License 1.1, and
  Bitstream Vera notices. See its [README](web/fonts/README.md) and accompanying
  `LICENSE-*.txt` files for the individual font origins and licenses.
- Rust dependencies are pinned in `Cargo.lock`; their upstream licenses remain
  applicable. A top-level project license does not supersede dependency licenses.

Before a first public release, confirm ownership/permission for the project's
original and imported code and preserve the bundled notices. A draft export
without a project license must remain a preparation artifact.
