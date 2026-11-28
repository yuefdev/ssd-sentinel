# Code signing policy

SSD Sentinel is preparing to use the SignPath Foundation open-source program. The current Windows releases are unsigned. The following attribution will apply once the Foundation approves SSD Sentinel and the project starts publishing signed releases.

> Free code signing provided by [SignPath.io](https://signpath.io/), certificate by [SignPath Foundation](https://signpath.org/).

## Signing scope

- The proposed scope is the Windows x64 NSIS installer, its SSD Sentinel application and generated uninstaller, and the SSD Sentinel executable inside the portable ZIP. The ZIP is distributed with a SHA-256 checksum and GitHub build provenance.
- SSD Sentinel signing applies to first-party artifacts built from the public repository by the [release workflow](.github/workflows/release.yml) on GitHub-hosted runners. The product name is `SSD Sentinel` and the Windows product and file versions match the release version.
- Bundled upstream `smartctl.exe` retains its upstream bytes. The release workflow verifies its pinned SHA-256. The project certificate is reserved for first-party files. This excludes `smartctl.exe`, NSIS and Tauri support DLLs, and the Microsoft WebView2 runtime and bootstrapper. See the [third-party notices](THIRD_PARTY.md).

## Team and approvals

| Role | Responsible maintainer |
| --- | --- |
| Author and committer | [Yusuf Efe (`@yuefdev`)](https://github.com/yuefdev) |
| Reviewer for external contributions | [Yusuf Efe (`@yuefdev`)](https://github.com/yuefdev) |
| Signing approver | [Yusuf Efe (`@yuefdev`)](https://github.com/yuefdev) |

The signing process requires multi-factor authentication for GitHub and SignPath access. The maintainer reviews external contributions before merging and manually approves each signing request. Production signing will use SignPath's GitHub trusted build system for origin verification. The release workflow publishes SHA-256 checksums and build provenance for the final public packages.

## Privacy and verification

See the [Privacy Policy](https://yuefdev.github.io/ssd-sentinel/privacy/) for local data, update checks, and the services involved. Verify Windows release packages using the [published checksums and attestations](README.md#cicd-staging--verifiable-releases). Report a concern privately using the [security policy](SECURITY.md).
