# Regole di scrittura della wiki

Come si scrive una pagina di questa wiki. Vincolante: una pagina che non rispetta
queste regole va corretta prima del merge.

## 1. Scopo di ogni pagina

Una pagina spiega **come un pezzo del sistema è fatto e perché esistono le scelte**.
Risponde a: cos'è, dove vive nel codice, come si comporta a runtime, quali contratti
ha con il resto, quali vincoli/limiti ha.

NON è:
- un changelog (→ `CHANGELOG/`),
- una spec di design o un piano (→ `docs/superpowers/`),
- un dump del codice (linka il codice, non incollarlo).

Se un'informazione si deduce dal codice in 10 secondi (firma di una funzione,
nome di un campo), **linkala**, non copiarla. La wiki documenta ciò che il codice
NON dice da solo: il perché, i contratti impliciti, gli invarianti, le insidie.

## 2. Una pagina = un concetto

Un componente o un sottosistema per file. Niente mega-pagine. Se una sezione cresce
oltre ~400 righe o copre due argomenti distinti, spezzala e collega.

## 3. Naming dei file

- Tutto **minuscolo, kebab-case**: `compositor.md`, `wasm-runtime.md`,
  `boot-phases.md`.
- Una cartella per area: `architecture/`, `components/`, `subsystems/`.
- Niente spazi, niente maiuscole, niente date nel nome (la data sta
  nell'intestazione).

## 4. Intestazione obbligatoria

Ogni pagina inizia con titolo H1 + un blocco di metadati:

```markdown
# <Titolo del componente>

> **Stato:** stub | bozza | stabile
> **Aggiornato:** AAAA-MM-GG
> **Fonti:** `kernel/src/.../file.rs`, `path/altro.rs`
> **Spec collegate:** docs/superpowers/specs/<...>.md (se esistono)
```

- **Stato** — `stub` (scheletro, da riempire), `bozza` (incompleto ma utile),
  `stabile` (rispecchia il codice, revisionato).
- **Aggiornato** — data ISO dell'ultima revisione del contenuto. Convertire sempre
  date relative in assolute.
- **Fonti** — i file sorgente che la pagina documenta. Chi aggiorna quel codice
  sa che deve guardare qui. Tieni la lista veritiera.
- **Spec collegate** — opzionale; la spec di design da cui nasce il componente.

## 5. Link al codice

- Linka i file con path **relativi alla root del repo**:
  `kernel/src/wasm/wt/wm.rs`. Quando indichi un punto preciso usa `file:riga`
  (es. `wm.rs:1530`), ma sappi che le righe scivolano: preferisci citare il **nome
  della funzione/tipo** (`Compositor::run`) che resta stabile.
- NON incollare blocchi di codice lunghi. Una firma o 3-4 righe per illustrare un
  contratto vanno bene; oltre, linka.
- Quando un numero di riga è citato, è valido al momento della revisione (vedi
  campo *Aggiornato*); non garantirlo per sempre.

## 6. Link interni

- Tra pagine: link markdown relativi (`[Compositor](components/compositor.md)`).
- Ogni pagina di componente linka indietro all'[indice](README.md) e alle pagine
  correlate. Collega in abbondanza: un link a una pagina non ancora scritta è ok,
  segnala cosa manca.

## 7. Struttura consigliata di una pagina di componente

1. **Cos'è** — una frase, poi un paragrafo.
2. **Dove vive** — file/moduli principali.
3. **Modello / tipi** — le strutture dati centrali e il loro ruolo.
4. **Comportamento a runtime** — il loop/flusso principale, passo per passo.
5. **Contratti** — ABI/host fn/protocolli con gli altri pezzi.
6. **Vincoli e limiti** — budget, invarianti, assunzioni (single-thread, ecc.).
7. **Insidie / note** — cose non ovvie che mordono chi tocca il codice.
8. **Vedi anche** — link correlati.

Adatta l'ordine al componente; queste sono le domande a cui rispondere, non un
modulo rigido.

## 8. Diagrammi

ASCII art dentro un blocco di codice. Niente immagini binarie (non versionano
bene, non si leggono in terminale). Schemi semplici a riquadri/frecce.

## 9. Tono e lingua

- **Lingua: italiano** (come CLAUDE.md, CHANGELOG, e queste pagine). I termini
  tecnici restano in inglese (compositor, store, surface, z-order, host fn).
- Diretto e denso. Niente riempitivi. Frasi che portano un fatto.
- Tempo presente per descrivere il comportamento ("il compositor compone…").
- Esatto sui nomi: usa i nomi reali di tipi/funzioni/campi del codice.

## 10. Manutenzione

- Quando cambi un componente nel codice, aggiorna la sua pagina **nello stesso
  lavoro** e bumpa *Aggiornato*.
- Se una pagina diverge dal codice e non puoi sistemarla subito, declassa lo
  **Stato** a `bozza` e annota cosa è stale, così il lettore è avvisato.
- Una entry in `CHANGELOG/` per ogni modifica alla wiki, come per il resto del
  repo (vedi CLAUDE.md).
