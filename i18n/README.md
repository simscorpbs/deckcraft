# Interface catalogs

`locales/<locale>/common.tsv` is a copy of storytold/craft-i18n `locales/pt-br/common.tsv` at commit
`77873ea8261e448a137984b0bb0b8d57ed0ec4e9` (craft-i18n main on 2026-11-08). Refresh it by copying the file again
and noting the new commit here.

`locales/<locale>/deckcraft.tsv` holds DeckCraft's own rows and the ones that must differ from the shared
file (ribbon tab names use the context `tab`). Format: `context<TAB>English<TAB>translation`, see the header of
`common.tsv`. How to add strings: the module doc of `crates/ui-egui/src/i18n.rs`.

`crates/ui-egui/build.rs` embeds these (or a craft-i18n checkout given as `CRAFT_I18N_DIR`).
