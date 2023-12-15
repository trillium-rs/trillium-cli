# Changelog
All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.1.1](https://github.com/trillium-rs/trillium-cli/compare/v0.1.0...v0.1.1) - 2023-12-15

### Other
- Merge pull request [#10](https://github.com/trillium-rs/trillium-cli/pull/10) from trillium-rs/deps
- update dependencies
- release

## [0.1.0](https://github.com/trillium-rs/trillium-cli/releases/tag/v0.1.0) - 2023-11-23

### Fixed
- fix ci
- fix a bug where the html-rewriter resulted in chunked content and also a content-length

### Other
- try release-plz
- Merge pull request [#4](https://github.com/trillium-rs/trillium-cli/pull/4) from trillium-rs/dependabot/github_actions/Swatinem/rust-cache-2.7.1
- Merge pull request [#3](https://github.com/trillium-rs/trillium-cli/pull/3) from trillium-rs/dependabot/github_actions/actions/checkout-4
- Merge pull request [#2](https://github.com/trillium-rs/trillium-cli/pull/2) from trillium-rs/dependabot/github_actions/actions/cache-3
- remove unnecessary joint license
- update readme
- touch up .github
- update deps and switch to clap
- update cli
- cargo.toml updates
- add top level help docs
- update deps
- use trillium 0.2.0
- update deps
- no docs for cli, it's a bin
- no mdbook in this repo
- split trillium-cli into its own repo
- Update nix requirement from 0.21.0 to 0.22.0
- remove a dbg and add a clippy to keep me from doing that again
- update deps
- (cargo-release) version 0.2.0
- enable the smol feature in order to build docs
- opt cli out of docs
- update dependencies
- add versions to the cli deps
- update deps
- deny missing docs everywhere
- more docs, DevLogger → Logger::new()
- client connector implementations
- update deps
- silence warnings on client
- finish documenting trillium sessions
- no need to spin up a multi-threaded executor
- remove all usage of Sequence
- partially document session handler
- upgrade everybody except webpki-roots
- begin documenting handlebars
- bump all deps (wip)
- propagate conn method renaming and fix tests
- cargo install trillium-cli installs a binary called trillium!
- cargo fixed
- 🎶 say my name, say my name 🎵
- docs and some reorganization
- udeps
- rename bin and use async-fs instead of async-io (for windows)
- rebase/merge-introduced mistake
- print out request headers
- cli tool: add client and proxying dev server
- add a conn-based outbound client and use it for proxy
- clippy devserver
- add LISTEN_FD support
- less debounce
- add in initial cli
