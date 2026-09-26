# Bundled terminal fonts

`UbuntuSansMono.woff2` is the Ubuntu Sans Mono variable font installed by
Ubuntu 26.04 (`fonts-ubuntu` 0.869+git20240321-0ubuntu2, TTF SHA-256
`507870a3fe5737eb5b02f366587d30607c02a5ad45d8fe94b3ee20a80ccad32a`), repackaged
as WOFF2 with fontTools (`TTFont(ttf).flavor = "woff2"`); glyphs, the `wght` axis
and metadata are unchanged, the transfer size halves.

See `LICENSE-Ubuntu-Font.txt` for the Ubuntu Font Licence 1.0.

`CascadiaMono.woff2` is the variable web font from Microsoft's official
[`Cascadia Code v2407.24`](https://github.com/microsoft/cascadia-code/releases/tag/v2407.24)
release. Its SHA-256 value is
`7d4986a68fbbd674c90cdd8cd5d4a28c698640d3472572b006d2978006049799`.

See `LICENSE-Cascadia-Code.txt` for the SIL Open Font License 1.1.

## Code block arrow fallback

`CodeSymbols.woff2` contains the U+2190–21FF arrows from DejaVu Sans Mono
2.37, including diagonal arrows absent from Cascadia Mono. Source TTF SHA-256:
`c805f9436dbc268644c1d9584f01a601a653e028e08fd74b9b949f6cf8304d88`.
Glyph outlines and advances are unchanged. Reproduce with fontTools:

```sh
pyftsubset DejaVuSansMono.ttf --unicodes=U+2190-21FF --name-IDs='*' \
  --name-legacy --name-languages='*' --flavor=woff2 --output-file=CodeSymbols.woff2
```

See `LICENSE-CodeSymbols.txt` for the Bitstream Vera license; DejaVu changes
are public domain.

Code blocks retain native inline text for continuous selection. The code-only
CSS faces match Cascadia's 1200/2048 em cell: DejaVu arrows use size-adjust
1200/1233, and full-em CJK glyphs use size-adjust 2400/2048. At the 12px code
size this makes Chinese about 14.06px, exactly two Latin cells, without per-glyph
containers. Geometric text rendering avoids independently rounded font advances
breaking those ratios at fractional zoom. Terminal font preferences are separate.
