# Changelog
All notable changes to this project will be documented in this file. See [conventional commits](https://www.conventionalcommits.org/) for commit guidelines.

- - -
## v0.2.0 - 2026-11-27
#### Build system
- ship AppImage, deb, rpm and tarball - (b48cd65) - dcog989
#### Miscellaneous Chores
- (**deps**) bump taiki-e/install-action from 2.90.3 to 2.90.8 - (66182f6) - dependabot[bot]

- - -

## v0.1.0 - 2026-11-27
#### Features
- (**build**) add user-level install recipe - (de58767) - dcog989
- (**core**) prune journal runs older than 60 days at startup - (6b301d7) - dcog989
- (**gui**) write the operational log beside the journal - (84ab80b) - dcog989
- (**gui**) role-tint the status palette and fill the content table - (7fbebde) - dcog989
- (**gui**) add About window opened by a logo button in the content footer - (af923ac) - dcog989
- (**gui**) keep the journal's ant behind the runs - (8837264) - dcog989
- (**gui**) apply straight from Rename now, without a confirmation - (4b3effd) - dcog989
- (**gui**) add icons to the add and rename buttons - (54f64f3) - dcog989
- (**gui**) sort, remove and clear runs in the journal - (3035f0d) - dcog989
- (**gui**) label the apply button "Rename Now" - (0332488) - dcog989
- (**gui**) frame the content table and rule off the column titles - (fcbc673) - dcog989
- (**gui**) tint every table row by its status - (7656d0d) - dcog989
- (**gui**) add browsed folders at once and commit typed paths with Enter - (5dc237d) - dcog989
- (**gui**) render timestamps in local time - (c9a338c) - dcog989
- (**gui**) expand a journal run to show what it renamed - (15c3d6b) - dcog989
- (**rules**) match a regex against the full path - (8e9096d) - dcog989
- (**rules**) add date rule with prefix and suffix stamps - (52c6740) - dcog989
- add transliterate to normalize rule - (665a9e9) - dcog989
- undo a single rename from the journal - (6ab22f9) - dcog989
- register the AppImage in the desktop menu - (9d69014) - dcog989
- rules + ui refined - (a022933) - dcog989
#### Bug Fixes
- (**build**) package from the workspace and the 64-bit libraries - (74895f3) - dcog989
- (**build**) render the AppImage icon from assets - (134fead) - dcog989
- (**core**) skip already reversed rows when undoing a run - (b66119c) - dcog989
- (**core**) repair the remaining lint errors - (2a30897) - dcog989
- (**core**) repair the remaining lint errors - (c7013d3) - dcog989
- (**core**) protect the journal from torn writes and unbounded reads - (e2165e2) - dcog989
- (**core**) close chunk 6 conflict and ordering gaps - (8d058eb) - dcog989
- (**core**) make planning linear, pure, and path-safe - (cdd013b) - dcog989
- (**core**) pass the parent path to starts_with explicitly - (269b1df) - dcog989
- (**core**) scan a picked folder link and survive unreadable dirs - (02dbd6c) - dcog989
- (**core**) keep accents and reject number overflow - (2cf0797) - dcog989
- (**core**) stage unicode case-only names and tighten validation - (6838d8f) - dcog989
- (**core**) reject reserved names and case-folded targets - (7bda46b) - dcog989
- (**core**) make undo and journal safe against real failures - (2b52a24) - dcog989
- (**editor**) drop the redundant hover text on a rule's on/off checkbox - (0101c57) - dcog989
- (**editor**) keep a card's size when a rule is toggled on or off - (320a6ee) - dcog989
- (**editor**) show a rule's hint at normal weight when the rule is on - (8c86959) - dcog989
- (**gui**) open paths passed by a file manager - (473b1f6) - dcog989
- (**gui**) fill the content column with the table and its spacer - (5abfd0b) - dcog989
- (**gui**) enclose the last row and pin the count under the table - (0037763) - dcog989
- (**gui**) pluralise the window's counts - (0b281de) - dcog989
- (**gui**) put the content count in its own bottom panel - (1a0da83) - dcog989
- (**gui**) give the rule card one hover target - (d38b72f) - dcog989
- (**gui**) drop a stat pass built for another plan - (60cba85) - dcog989
- (**gui**) close the content table's header rule on its row - (bd4f2ad) - dcog989
- (**gui**) frame the content table's titles as well as its rows - (d0200f7) - dcog989
- (**gui**) keep the preview when an enabled replace rule has no find text - (f5c9401) - dcog989
- (**gui**) give each journal remove chip its own id - (6a605d5) - dcog989
- (**gui**) repair the history warning import and the deprecated ui id - (8cadb56) - dcog989
- (**gui**) register the desktop entry atomically and after the lock - (b386c14) - dcog989
- (**gui**) make journal removal and run reports explicit - (5f96c2e) - dcog989
- (**gui**) make the preview table readable and keyboard reachable - (de6b615) - dcog989
- (**gui**) keep run failures and the shown plan honest - (5e80707) - dcog989
- (**gui**) finish the journal audit follow-ups - (ab94cfb) - dcog989
- (**gui**) keep a cleared scan out and trim preview work - (2196a08) - dcog989
- (**gui**) label a reserved name as reserved - (13178d0) - dcog989
- (**gui**) refuse to start when the instance lock cannot be taken - (c68423f) - dcog989
- (**gui**) keep actions on fresh state and bound history work - (0e39237) - dcog989
- (**gui**) drop an unknown saved rule instead of losing the whole pipeline - (b7d7d1b) - dcog989
- (**gui**) let the first row's stripe reach into the gap above it - (39cde06) - dcog989
- (**gui**) restore symmetric cell padding from the base style - (69f903f) - dcog989
- (**gui**) align table columns on one font and vertical centre the cells - (f7b311b) - dcog989
- (**gui**) drive the whole rule toggle's hover from one shared rect - (97db97f) - dcog989
- (**gui**) highlight a rule's checkbox when its name is hovered - (6e35227) - dcog989
- (**gui**) disable rule fields while the rule is off - (3d98370) - dcog989
- (**gui**) resolve the window icon for local Wayland runs - (edd12d1) - dcog989
- (**ui**) refine cards design ++ - (52b653e) - dcog989
- recover staged names, harden undo and drop the broken overwrite opt-in - (709191f) - dcog989
#### Documentation
- add screenshot to readme - (7d3481d) - dcog989
- name the sort control the window actually shows - (f99dcbc) - dcog989
- state the sequence numbering order - (8fce002) - dcog989
#### Tests
- (**core**) avoid cloning a single path into a slice - (9ea09d9) - dcog989
- (**gui**) pick the confirmation's Undo by position, not a panel-width guess - (e92d704) - dcog989
#### Build system
- require the committed lockfile when building - (127f2b4) - dcog989
#### Continuous Integration
- add check, packaged and attested release workflows - (e7cb0db) - dcog989
#### Refactoring
- (**core**) state the lock-file open intent explicitly - (ce15800) - dcog989
- (**core**) trim unused exports and correct the I/O contract - (440a05c) - dcog989
- (**core**) drop serialization nothing reads - (85f7b20) - dcog989
- (**core**) move rule text shaping into rule::text - (8c1fde3) - dcog989
- (**gui**) replace the text colour constants with a base-plus-alpha ramp - (b7ca35c) - dcog989
- (**gui**) move the shared UI into the base style - (5554074) - dcog989
- (**gui**) replace Browse menu with add file and folder buttons - (7185cd0) - dcog989
- (**gui**) tile row tints instead of centring them on the row - (6fa14f4) - dcog989
- (**rules**) share placed-token writing and placement UI - (07cf725) - dcog989
- (**style**) derive bright, dim and disabled from one opacity ladder - (f5f033c) - dcog989
- resolve every XDG directory in one place - (2fb17be) - dcog989
#### Miscellaneous Chores
- (**build**) add an update recipe - (964fd69) - dcog989
- (**build**) add a justfile for the common dev tasks - (5ca582b) - dcog989
- (**gui**) inherit the workspace MSRV - (af5597b) - dcog989
- format - (8ebf4a9) - dcog989
- refine tooltips - (5ba4b7e) - dcog989
- updates - (18b49f0) - dcog989
- sync with scaffold - (b98f2cd) - dcog989
- tidy, agents.md refined - (e7a4e8c) - dcog989
- init - (13e7b61) - dcog989
#### Style
- (**gui**) draw the rule checkbox's tick in the accent - (9534235) - dcog989
- (**gui**) put each rule's checkbox between its icon and name - (c72891b) - dcog989
- (**gui**) put each rule's checkbox before its name - (cb9cf83) - dcog989
- (**gui**) apply rustfmt to the subdirectory tooltip - (1a7ea46) - dcog989
- (**gui**) put the entries count under the content table - (9a1ae38) - dcog989
- apply rustfmt 1.100 - (505a5de) - dcog989
- format with rustfmt 1.11 - (1899581) - dcog989

- - -

Changelog generated by [cocogitto](https://github.com/cocogitto/cocogitto).