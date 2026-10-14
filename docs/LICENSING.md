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
| `vendor/upleft` | Its [MIT license](../vendor/upleft/LICENSE) has exceptions for individual components. In particular, [`upleft-elk`](../vendor/upleft/crates/elk/Cargo.toml) retains [EPL-2.0](../vendor/upleft/crates/elk/LICENSE). Omperor enables the Mermaid renderer's default ELK feature. |
| `vendor/sidestep` | [MIT](../vendor/sidestep/LICENSE-MIT) OR [Apache-2.0](../vendor/sidestep/LICENSE-APACHE), with separate notices for incorporated third-party material. |
| Math font bundle | SwiftMath's font tables and code keep their [MIT notice](../vendor/upleft/vendor/downright/Vendor/SwiftMath/Sources/SwiftMath/mathFonts.bundle/LICENSE). Latin Modern Math and TeX Gyre Termes Math are under the [GUST Font License](../vendor/upleft/vendor/downright/Vendor/SwiftMath/Sources/SwiftMath/mathFonts.bundle/GUST-FONT-LICENSE.txt), which refers to LPPL-1.3c-or-later. XITS Math is under the [STIX Font License](../vendor/upleft/vendor/downright/Vendor/SwiftMath/Sources/SwiftMath/mathFonts.bundle/OFL.txt), which is based on SIL OFL 1.1. Asana, Euler, Fira, Garamond, KpMath (Light and Sans), Lete Sans, Libertinus and Noto Sans Math are under SIL OFL 1.1, stated in each font's name table. Preserve the fonts' copyright notices and reserved font names. |
| Other Rust and native dependencies | Follow each package's license and attribution terms, including those of bundled native libraries and data. Inspect the resolved dependencies for each release target and feature set. |

The dependency versions reviewed were OMP
`12d59c0937e1e5ed1b52dee3d237279521fe847f`, Upleft
`4b28a9e758dc1f8832d1d902857dc2348931516b`, and Sidestep
`5e5c08b19ceaa84e3add0fc4019bf1ffbdfc98bb`.

EPL-2.0's source and notice obligations still apply to `upleft-elk`, including
modifications to that component. Its source must remain available under EPL-2.0,
and distributions must tell recipients how to obtain it. Omperor's separate
application code is offered under the root dual license; that does not turn
the ELK implementation into MIT/Apache code. See
[EPL-2.0 sections 1 and 3](https://www.eclipse.org/org/documents/epl-2.0/EPL-2.0.txt).

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
   bundled data. Include matching EPL/MPL source or working source locations.
   `scripts/build-app.sh` currently preserves the font bundle's notices but
   does not assemble an app-wide license and attribution bundle.

The root Cargo dependency inventory was checked with
`cargo metadata --locked --offline --format-version 1`. It does not cover the
complete separately built OMP helper or all bundled assets. Refresh the
inventory against both Cargo lockfiles, the helper, and the packaged files
before distributing a release.
