# Notes for agents

## Log improvement ideas in TODO.md

When an improvement, follow-up or known gap comes up while working (something worth doing but
outside the task at hand), add it to `TODO.md` rather than leaving it in the conversation. File it
under *Worth doing*, *Maybe* or *Considered, probably not*, and say which files it touches, roughly
how big it is, and why (with measurements, if there are any). Tick off or remove items once they're
done.

## Versioning

The version (`[workspace.package]` in `Cargo.toml`; the flake reads it from there) follows SemVer
as Cargo reads it before 1.0. The level says what an upgrade can break, not how big the change is:

- **0.MINOR**: something that worked stops working, after upgrading or going back. Examples: a deck
  database `SCHEMA` bump (its migration is one-way); a removed or renamed flag, subcommand or config
  key, or one whose meaning changed; a changed decklist export format; state files moved without
  migrating them.
- **0.x.PATCH**: anything else a user notices. Examples: features, however large; fixes; and
  `DECK_PARSER_VERSION`/`SCORER_VERSION` bumps (stored decks are re-read and re-scored
  automatically, in both directions).
- **No bump**: refactors, tests, docs, TODO.md.

Bump once per batch, meaning the commits that land together (one commit plan). The first commit in
the batch that needs a bump sets the highest level the batch needs, in `Cargo.toml` and
`Cargo.lock`. Later commits in the batch don't bump again. Tag the batch's last commit `vX.Y.Z`.

`CHANGELOG.md` has a section per version, newest first, with one bullet per change, and more only
when a change touches many moving parts. A minor version's section also says what breaks and how to
go back.
