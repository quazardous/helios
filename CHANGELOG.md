# Changelog

All notable changes to this project will be documented in this file.

> This is a curated, human-readable record -- **not a commit log**. Each
> entry says *what changed and why it matters to a user*, in plain
> language, not *how* it was implemented. Skip internal refactors.
>
> **House style** for editors:
> - One short bullet per change. Multi-paragraph entries are only for
>   the major changes a user really needs to read in full.
> - No internal tracker IDs (`#NNN`, `PROJ-123`) unless that tracker
>   has a public link -- they're noise otherwise. Mention the change,
>   not the ticket.
> - **Version bump = SemVer**: any `### Added` entry is at least
>   MINOR; `### Fixed` alone is PATCH; breaking change is MAJOR.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- Project roadmap describing the vision, concepts and phases.
- Design drafts: module contract, internal architecture, testing strategy.
- First Rust crates: the core (module contract, graph, content-addressed
  evaluation that never computes the same content twice, event bus) and a
  test kit generating contract tests from a module's manifest.
- darktable's exposure module, manual mode, as the first ported module.
- A headless node and a command line to observe it: state snapshots, bus
  events and waiting on an event, read-only, so scripts and agents never
  have to sleep or parse logs.
- UX research: user feedback on darktable's interface and its equivalents,
  real workflows, and scenarios for the GUI.
