# Maintainers

This is the public, authoritative list of people who can merge to
`makakoo-os`. [`CONTRIBUTING.md`](../CONTRIBUTING.md) describes the
governance model in one paragraph; this file names the humans and the
escalation path behind it.

## Current maintainers

| Name                    | GitHub        | Areas                                                                 |
| ----------------------- | ------------- | --------------------------------------------------------------------- |
| Sebastian Schkudlara    | [@rschumann](https://github.com/rschumann) | Everything. Kernel, adapters, install/release path, security gates. BDFL tiebreaker for v0.x. |

A single maintainer is a fact of a young project, not a goal. The table
grows the moment a contributor has earned it (see *Becoming a maintainer*
below), and [`.github/CODEOWNERS`](../.github/CODEOWNERS) is structured so
adding handles is a one-line change per area.

## What a maintainer does

- **Merges.** No one merges their own non-trivial PR without a second
  maintainer's approval once there is a second maintainer. Until then,
  Sebastian self-merges but every change still lands through a PR with a
  green CI checkmark — never a direct push to `main`.
- **Owns CI green.** A red `main` is a maintainer's problem to fix or
  revert within the day, not a contributor's.
- **Triages security reports.** See [`SECURITY.md`](../SECURITY.md). The
  target is a 72-hour acknowledgement.
- **Cuts releases.** The release runbook is
  [`docs/RELEASING.md`](RELEASING.md); the signing runbook is
  [`docs/RELEASE_SIGNING.md`](RELEASE_SIGNING.md). Tags are pushed only by
  a maintainer.

## How decisions get made

Decisions live in public — issues, PRs, and the GitHub Discussions board.
The order of resolution:

1. **Lazy consensus.** Most changes need no vote. Open a PR; if no
   maintainer objects, it merges.
2. **72-hour vote.** A disagreement a comment thread can't resolve goes to
   a lazy-consensus vote on a GitHub Discussion. Silence after 72 hours is
   assent.
3. **BDFL tiebreaker.** If a vote deadlocks, Sebastian breaks the tie for
   the v0.x line. v1.0 replaces the tiebreaker with a maintainer majority.

No company, no VC, no private roadmap doc that overrides the public one.

## Becoming a maintainer

There is no application form. The path is mechanical and earned:

1. Land a handful of non-trivial PRs that needed little rework.
2. Review other people's PRs usefully — concrete suggestions, caught
   regressions, cross-platform thinking.
3. An existing maintainer nominates you in a GitHub Discussion. Lazy
   consensus over 72 hours confirms it.

New maintainers get added to this table, to `.github/CODEOWNERS` for their
area, and to the GitHub team that carries merge rights.

## Stepping down

Maintainers who go quiet for a release cycle get moved to an *Emeritus*
section here (added when the first one does) — no drama, no loss of credit,
and the door stays open to come back.

## Security

Never report a vulnerability here or in a public issue. Follow
[`SECURITY.md`](../SECURITY.md): a private GitHub security advisory is the
preferred channel.
