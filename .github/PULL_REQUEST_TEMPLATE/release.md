<!--
Release: staging into main, by the maintainer. Title: "Release 0.2.0".
Merging it builds the installers after Check passes, publishes v0.2.0 with the list below as
its notes, and updates the website. See CONTRIBUTING.md, "Releasing".
-->

## Release 0.0.0

<!-- The commits below come in automatically as the release notes; add anything users should
     know first (a new setting, something that changed). -->

## Before merging

- [ ] Version bumped on staging in `Cargo.toml` and `crates/lody-app/tauri.conf.json`
- [ ] Check is green on staging (Linux, Windows, macOS)
- [ ] Tried the app from staging on at least one system

## After merging

- [ ] Build finished, and the release has all its installers
- [ ] The website offers the new version
- [ ] staging put back on main: `git fetch origin && git push --force-with-lease origin origin/main:staging`
