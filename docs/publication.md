# Source publication

OpenFig is a personal macOS/zsh fork, supplied as-is without a support or release schedule. Preserve [MIT](../LICENSE.MIT), [Apache-2.0](../LICENSE.APACHE), [NOTICE](../NOTICE) and every component's copyright/license notices. The upstream dual-licensed code is redistributed under its MIT option; dependencies keep their own terms. Public source availability does not imply endorsement, trademark clearance, an Apple notarization, or a warranty.

## Licenses and assets

Vendored code is attributed in NOTICE. `python3 scripts/license-notices.py` regenerates the dashboard's complete notices from installed, locked macOS dependencies; `--check` detects drift. It includes development tools, so its list is broader than a runtime bill of materials. Supplemental license texts use pinned upstream URLs in [licenses/sources.json](../licenses/sources.json). Some packages omit license texts from their published archives; the supplemental source may be a later upstream revision, explicitly identified there.

MPL-2.0 dependencies are file-level copyleft, not permissive licenses. When distributing their binaries, provide the corresponding covered source and license information; when modifying their covered files, publish those modifications under MPL-2.0. The generated notices identify exact source-package URLs. Preserve Apache NOTICE, BSD, ISC, Unicode, Blue Oak and other applicable notices too. Do not assume the root MIT license covers every dependency or asset. See the [MIT terms](https://opensource.org/license/mit), [Apache terms](https://www.apache.org/licenses/LICENSE-2.0) and [MPL guidance](https://www.mozilla.org/en-US/MPL/2.0/FAQ/).

The separate `withfig/themes` repository did not declare a license at review time. Its copied payloads and historical Amazon Ember font files with separate embedded EULA references are excluded from this repository's publication history. Historical code remains, but those historical asset-dependent builds are not self-contained. The private recovery archive retains the original history; it must never be uploaded. Built-in themes and the replacement application icon remain available. Vendor integration names/icons are identifiers, not an endorsement or a separate trademark grant.

`objc2` publishes its Apple-SDK provenance caveat alongside its license; the supplemental notices preserve it. OpenFig does not bundle Xcode or macOS frameworks. Build with your own licensed SDK. Software license review is not a determination that every trademark, patent, SDK or distribution-policy question is settled.

## Name and distribution channel

An unrelated Figma tooling project already uses [OpenFig](https://openfig.org/). This terminal fork does not claim affiliation or exclusive rights to that name. Distinguishing the projects in README reduces confusion but is not trademark clearance; review the public-facing name before promoting a release.

Source hosting and Mac App Store submission are different decisions. OpenFig still uses the private AX window-ID SPI documented in [architecture](architecture.md). Apple's [App Review Guidelines](https://developer.apple.com/app-store/review/guidelines/) require public APIs for App Store apps. Publishing source does not establish App Store eligibility; do not promise it or infer that a local signature implies notarization.

## Privacy and publication checks

Before changing visibility, inspect all branches, tags and historical file contents, not just HEAD. Run `python3 scripts/check-publication.py --history` and a full-history secret scan such as `gitleaks git --log-opts='--all --full-history' --redact=100`. Inspect findings; CLI documentation contains example keys, so a raw hit is not proof of a live credential. Never use a broad exclusion for an entire specs directory. Scanners cannot prove the absence of every secret or personal detail.

Do not commit session links, prompts, local logs, credentials, signing material, app backups, `bundle-id.local` or `themes.local/`. Preserve third-party author and copyright attribution. Use a pseudonym or GitHub noreply address for your own author/committer identity. Review images and other binary assets as well as text.

History cleanup changes commit IDs and invalidates signatures on rewritten objects. Keep an access-restricted backup outside the repository, verify every transformed commit/tree against the original, and update remote branches with explicit force-with-lease expectations. Do not merge or push an old clone afterward. GitHub can retain old objects and cached commit views after a force-push; reachable-ref cleanup is not server-side erasure. Resolve any remaining access to old sensitive objects with GitHub before publication. See [GitHub's removal procedure](https://docs.github.com/en/authentication/keeping-your-account-and-data-secure/removing-sensitive-data-from-a-repository).

The repository stays private until visibility is explicitly changed. A public release can be copied permanently. Binary distribution additionally requires reviewing the exact built artifact, all bundled notices, custom assets and signing/notarization choices; source checks alone do not certify a binary release.
