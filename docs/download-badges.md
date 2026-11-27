# Download badge preview

Sample counts for a layout preview. Each badge represents installer downloads. A person who downloads twice counts twice.

| Style | Total | macOS | Linux | Windows |
| --- | --- | --- | --- | --- |
| `flat` | ![Total downloads, sample count 1,250](https://img.shields.io/badge/Downloads-1%2C250-2E7D32?style=flat) | ![macOS downloads, sample count 540](https://img.shields.io/badge/macOS-540-333333?style=flat&logo=apple&logoColor=white) | ![Linux downloads, sample count 210](https://img.shields.io/badge/Linux-210-FCC624?style=flat&logo=linux&logoColor=white) | ![Windows downloads, sample count 500](https://img.shields.io/badge/Windows-500-0078D4?style=flat) |
| `flat-square` | ![Total downloads, sample count 1,250](https://img.shields.io/badge/Downloads-1%2C250-2E7D32?style=flat-square) | ![macOS downloads, sample count 540](https://img.shields.io/badge/macOS-540-333333?style=flat-square&logo=apple&logoColor=white) | ![Linux downloads, sample count 210](https://img.shields.io/badge/Linux-210-FCC624?style=flat-square&logo=linux&logoColor=white) | ![Windows downloads, sample count 500](https://img.shields.io/badge/Windows-500-0078D4?style=flat-square) |
| `plastic` | ![Total downloads, sample count 1,250](https://img.shields.io/badge/Downloads-1%2C250-2E7D32?style=plastic) | ![macOS downloads, sample count 540](https://img.shields.io/badge/macOS-540-333333?style=plastic&logo=apple&logoColor=white) | ![Linux downloads, sample count 210](https://img.shields.io/badge/Linux-210-FCC624?style=plastic&logo=linux&logoColor=white) | ![Windows downloads, sample count 500](https://img.shields.io/badge/Windows-500-0078D4?style=plastic) |
| `for-the-badge` | ![Total downloads, sample count 1,250](https://img.shields.io/badge/Downloads-1%2C250-2E7D32?style=for-the-badge) | ![macOS downloads, sample count 540](https://img.shields.io/badge/macOS-540-333333?style=for-the-badge&logo=apple&logoColor=white) | ![Linux downloads, sample count 210](https://img.shields.io/badge/Linux-210-FCC624?style=for-the-badge&logo=linux&logoColor=white) | ![Windows downloads, sample count 500](https://img.shields.io/badge/Windows-500-0078D4?style=for-the-badge) |
| `social` | ![Total downloads, sample count 1,250](https://img.shields.io/badge/Downloads-1%2C250-2E7D32?style=social) | ![macOS downloads, sample count 540](https://img.shields.io/badge/macOS-540-333333?style=social&logo=apple&logoColor=white) | ![Linux downloads, sample count 210](https://img.shields.io/badge/Linux-210-FCC624?style=social&logo=linux&logoColor=white) | ![Windows downloads, sample count 500](https://img.shields.io/badge/Windows-500-0078D4?style=social) |

## Using real counts

After you attach the installers to a public MyTube release, this badge shows total downloads across its release assets.

```md
![Release downloads](https://img.shields.io/github/downloads/mo1ein/mytube/total?style=flat-square&label=Downloads&color=2E7D32)
```

For a platform badge, use the exact published filename. The Windows x64 archive for the first release is `mytube-1.0.0-windows-x64.zip`.

```md
![Windows downloads](https://img.shields.io/github/downloads/mo1ein/mytube/mytube-1.0.0-windows-x64.zip?style=flat-square&label=Windows&color=0078D4&displayAssetName=false)
```

Shields matches one filename across releases. A single macOS or Linux total covering multiple architectures or versioned filenames requires summing the download counts for those assets. [Shields.io release badges](https://shields.io/badges/git-hub-downloads-specific-asset-all-releases)
