# 44 — `.gitattributes` + line-ending normalization (LF)

**Data:** 2026-05-28

## Cosa

- Aggiunto `.gitattributes` con `* text=auto eol=lf` + glob espliciti per `.rs`,
  `.toml`, `.lock`, `.md`, `.ld`, `.conf`, `.sh`, `Makefile`, e binary glob per
  immagini/ISO/qcow2.
- `git add --renormalize .` ha riscritto **solo** `kernel/src/main.rs` (era
  CRLF nell'index); il resto del repo era già LF.

## Perché

Lo working tree Windows tendeva a riscrivere i file in CRLF al `git add`,
producendo diff +N/-N inutili sul `main.rs` a ogni commit. `.gitattributes`
forza l'index a LF indipendentemente dal core.autocrlf locale, eliminando il
rumore per i commit successivi.

## File toccati

- .gitattributes (nuovo)
- kernel/src/main.rs (rinormalizzato CRLF→LF, contenuto invariato)
- CHANGELOG/44-26-05-28-gitattributes-eol.md
