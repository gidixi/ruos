# 120 — run-test asserisce lo smoke PCI

**Data:** 2026-05-29

## Cosa
`run-test` ora asserisce, oltre al sentinel shell, `pci init ok devices>=1` e
`xhci @ ...`, con tag di fallimento distinti (`TEST_FAIL_SHELL`/`_PCI`/`_XHCI`).

## Perché
Regression gate per lo Step 13: un fallimento di enumerazione PCI o la sparizione
del controller xHCI fanno fallire il test invece di passare silenziosamente.

## File toccati
- Makefile
