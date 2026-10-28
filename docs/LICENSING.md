# Licensing

Original Omperor code, documentation, and assets are available under
**MIT OR Apache-2.0**: recipients may choose either license. See the root
[license statement](../LICENSE), [MIT license](../LICENSE-MIT), and
[Apache license](../LICENSE-APACHE). Contributions use the same dual license
unless explicitly stated otherwise.

This choice applies to Omperor's own material. Dependencies and third-party
material retain their existing terms, including material appearing as context
in patches. A root license does not establish permission for material whose
provenance is unresolved.

## Dependency boundaries

The following are the principal boundaries checked on 2026-10-06. This is a
release-preparation record, not a complete binary attribution bundle.

| Component | Applicable licenses and notices |
| --- | --- |
| `vendor/oh-my-pi` | Its [MIT license](../vendor/oh-my-pi/LICENSE) applies to first-party OMP code unless a package says otherwise. Its [third-party notices](../vendor/oh-my-pi/THIRD-PARTY-NOTICES.txt) include package-specific terms, MPL-2.0 dependencies, Python notices, and explicit exclusions. |
| `vendor/sekka` | Aizome and its toolkit, `aizome-kit` (`desktop/aizome`), are [MIT](../vendor/sekka/desktop/aizome/Cargo.toml) OR Apache-2.0. Sekka as a whole carries an [Apache-2.0 license](../vendor/sekka/LICENSE) and a [NOTICE](../vendor/sekka/NOTICE) for adapted material. |
| `vendor/sekka/vendor/sidestep` | [MIT](../vendor/sekka/vendor/sidestep/LICENSE-MIT) OR [Apache-2.0](../vendor/sekka/vendor/sidestep/LICENSE-APACHE), with separate notices for incorporated third-party material. |
| Syntax highlighting | `syntect` is MIT. `two-face` is MIT OR Apache-2.0 and bundles `bat`'s syntax definitions, each under its own license (MIT, Apache-2.0, BSD and others), listed with the crate. |
| Math | The RaTeX crates (`ratex-*`) are MIT. `ratex-katex-fonts` embeds KaTeX's fonts, which are under the SIL Open Font License 1.1 (`fonts/FONT_NOTICE.txt` and `fonts/OFL.txt` in that crate). `skrifa` is MIT OR Apache-2.0. |
| Diagrams | `merman` and the parts of it Omperor uses (`merman-core`, `merman-render`, `dugong`, `manatee`) are MIT OR Apache-2.0; `merman-unicode-linebreak` is Apache-2.0. Omperor leaves out merman's ELK layout, whose port (`merman-elk-layered`) is EPL-2.0. `usvg` is Apache-2.0 OR MIT. |
| Fonts | Omperor bundles none: Aizome's session provides Archivo, IBM Plex Mono and Noto Sans Symbols 2 (SIL OFL 1.1). The dev shell links the same files for tests (`nix/aizome-fonts.nix`). |
| Other Rust and native dependencies | Follow each package's license and attribution terms, including those of bundled native libraries and data. Inspect the resolved dependencies for each release target and feature set. |

The dependency versions reviewed were OMP
`12d59c0937e1e5ed1b52dee3d237279521fe847f`, Sekka
`4dc885718ce48fd5555adff48908ab517efb62fe` (with Omperor's toolkit
additions on top), and Sidestep `dae957e913376a26e642d6abee533eb755fb9092`.

## Before a public release

1. Resolve or exclude OMP's material with unestablished redistribution rights.
   At the pinned revision, its notices identify **90 provider logo files** in
   `crates/tui/assets/login` and **21 recovered Cursor/Devin protocol schemas**
   in `fixtures/llm-oracle/vendor-schemas`. Their provenance manifests record
   the affected files. OMP's TUI build generates `include_bytes!` references
   to the logos, so check executable contents as well as source archives;
   hiding a UI surface does not establish that its assets are absent.
2. Keep the pinned OMP source and Omperor's applied patches obtainable by
   recipients. `.gitmodules` names the public fork
   `https://github.com/bitemyapp/oh-my-pi.git`, and the patches live in
   `patches/oh-my-pi`; every new pin must be pushed there before it is
   committed, and a binary release should state where both are found.
3. Ship the license texts, copyright notices, and source-availability statements
   required by the actual release payload. Cover the Omperor executable,
   patched OMP helper, embedded Python, native libraries, fonts, and other
   bundled data. Include matching MPL source or working source locations.
   `just install` does not yet assemble a license and attribution bundle.

The root Cargo dependency inventory was checked with
`cargo metadata --locked --offline --format-version 1`. It does not cover the
complete separately built OMP helper or all bundled assets. Refresh the
inventory against the Cargo lockfile, the helper, and the packaged files
before distributing a release.
