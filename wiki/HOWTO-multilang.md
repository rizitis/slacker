# slacker HOWTO

Scegli la tua lingua / Выберите язык / Elige tu idioma / 选择语言 / Choisissez votre langue :

- [Italiano](#it)
- [Русский](#ru)
- [Español](#es)
- [中文](#zh)
- [Français](#fr)

Commands, flags, paths, identifiers and the program's own output are kept in the
original (English/code) on purpose; only the explanatory prose is translated.

---

<a id="it"></a>
# Italiano

Una guida pratica e basata su esempi a `slacker`, un gestore di pacchetti binari
per Slackware che unisce `slackpkg` e `slackpkg+` in un solo strumento.

Ogni esempio qui sotto è un comando reale. Le azioni che modificano il sistema
(install, upgrade, remove, ...) richiedono root; le interrogazioni (search, info,
file-search, ...) no.

## Indice

1. Prima configurazione
2. Mantenere aggiornati i metadati
3. Ricerca, ispezione ed elenco dei repository
4. Installazione
5. Aggiornamento
6. Reinstallazione e rimozione
7. Repository interi e build tag (selettori `@`)
8. Gestione dei repository (aggiungere, rimuovere, tag, fiducia)
9. Scaricare senza installare
10. Congelare i pacchetti (blacklist)
11. Template
12. Pulizia
13. Flag globali e codici di uscita
14. Flussi di lavoro comuni
15. Verifica dei pacchetti

## 1. Prima configurazione

La configurazione si trova in `/etc/slacker/` (sovrascrivibile con
`--config-dir`). Copia i template forniti e modificali:

```


```

Scegli esattamente un mirror in `/etc/slacker/mirrors` (nessuno è attivo per
impostazione predefinita; due o più righe attive sono un errore):

```
# /etc/slacker/mirrors  - uncomment ONE line
https://slackware.uk/slackware/slackware64-current/
```

Dichiara i tuoi repository in `/etc/slacker/repos`. I repository binari prendono
un URL (o la parola chiave `mirror` per quello ufficiale) e devono avere priorità
**distinte**; vince la priorità più alta:

```
# priority  name        url|mirror                                            [flags]
100         slackware   mirror                                                official
90          extras      https://mirror.nl.leaseweb.net/slackware/slackware64-current/extra   subtree
80          conraid     https://slackers.it/repository/slackware64-current
60          alienbob    https://slackware.nl/people/alien/sbrepos/current/x86_64
```

I flag (in qualsiasi ordine) vanno dopo l'URL: `official` (il repository
tracciato), `subtree` (un sottoalbero della distribuzione Slackware — vedi
sotto), `immutable` (un repository i cui pacchetti `clean-system` non rimuove
mai) e `verify=...` (override della verifica per repository).

I quattro sottoalberi della distribuzione Slackware — **`extra`, `patches`,
`testing`, `pasture`** — **devono sempre portare il flag `subtree`** (in
qualsiasi posizione dopo l'URL). Il loro `PACKAGES.TXT` elenca le posizioni dei
pacchetti relative alla radice della distribuzione, quindi senza `subtree` i loro
pacchetti non si scaricano (un segmento di percorso raddoppiato); con esso, i
pacchetti e `GPG-KEY` vengono presi dall'URL genitore (radice) mentre i metadati
provengono ancora dall'URL del repository stesso. Per questi quattro repository
non è facoltativo.

Facoltativamente, aggiungi righe di priorità per tag affinché i pacchetti da
sorgenti/locali non vengano mai migrati o retrocessi da `upgrade-all`:

```
100         SBo         _SBo
100         local       _rtz
```

Importa una volta le chiavi GPG dei repository, poi aggiorna e controlla la
configurazione:

```
slacker update gpg
slacker update
slacker status                  # conferma che la configurazione sia sana e segnala cosa correggere
```

`update gpg` fissa (pin) la chiave di ogni repository (trust on first use). Per un
repository `subtree` fissa la stessa chiave Slackware dalla radice, ed è proprio
ciò che consente alla verifica GPG per pacchetto di riuscire per i pacchetti
`extra/`/`testing/`/`patches/` durante l'installazione.

## 2. Mantenere aggiornati i metadati

```
slacker update                  # aggiorna PACKAGES.TXT/CHECKSUMS, verifica GPG
slacker update gpg              # (re)importa le chiavi GPG, poi aggiorna
slacker check-updates           # controlla ogni repository; esce con 100 se ci sono aggiornamenti in sospeso
slacker show-changelog          # mostra il ChangeLog in cache (con pager su TTY)
slacker show-changelog conraid  # ChangeLog di un repository indicato (scaricato su richiesta)
```

## 3. Ricerca, ispezione ed elenco dei repository

```
slacker search firefox          # trova un pacchetto per nome esatto (senza distinzione maiuscole/minuscole)
slacker info bash               # candidati per repository + versione installata
slacker file-search bin/bash    # quale pacchetto contiene questo file (usa MANIFEST)
slacker list-repos              # repository: priorità, conteggio installati, verify, flag
slacker status                  # controllo di salute dell'intera configurazione; dice cosa correggere
```

`info` mostra quale repository vince per priorità. Per esempio, se `ffmpeg`
esiste in più repository, il candidato è quello a priorità più alta; fissane un
altro con `repo:name` (vedi sotto).

## 4. Installazione

```
slacker install vlc                     # un pacchetto (+ la sua catena di .dep)
slacker install vlc mpv obs-studio      # diversi insieme
slacker install conraid:ffmpeg          # forza la build di conraid (pin)
slacker --dry-run install vlc           # solo anteprima, non cambia nulla
slacker --no-deps install vlc           # salta la risoluzione delle dipendenze
slacker -y install vlc                  # assume "sì" a tutti i prompt
```

Se un pattern corrisponde a più di un pacchetto, slacker stampa un elenco
numerato:

```
slacker install python
# 'install' matched 12 packages:
#   1) [slackware] python-build-...
#   2) [slackware] python-cffi-...
#   ...
# Enter numbers to install (e.g. 1 3 5 or 2-4), Enter for all, 'n' to cancel:
```

I pacchetti già installati vengono rifiutati da `install` (usa `upgrade` o
`reinstall`).

**Le dipendenze sono una funzione di terze parti, non ufficiale.** I repository
propri di Slackware — `slackware` e i sottoalberi `extra`/`patches`/`testing`/
`pasture` — non forniscono né si aspettano informazioni sulle dipendenze:
un'installazione completa di **tutti** i set di pacchetti Slackware è il
prerequisito ufficiale, quindi ogni dipendenza è considerata già presente, e
slacker non esegue alcuna risoluzione delle dipendenze per essi (non serve). I
repository indipendenti come `alienbob` e `conraid` forniscono file `.dep` per
pacchetto; slacker li legge **solo per i repository che li forniscono**, tirando
dentro le dipendenze mancanti dallo stesso repository. Quindi un
`install`/`upgrade` dall'albero ufficiale non risolve nulla, mentre uno da un
repository di terze parti con file `.dep` sì. Disattivalo ovunque con `--no-deps`
(per esecuzione) o `RESOLVE_DEPS=no` (in `slacker.conf`).

## 5. Aggiornamento

```
slacker upgrade vlc             # aggiorna pacchetti specifici
slacker upgrade-all             # aggiorna tutto ciò che ha una revisione più recente
slacker --dry-run upgrade-all   # anteprima dell'intero piano di aggiornamento
slacker -y upgrade-all          # senza prompt
```

`upgrade-all` rispetta priorità e build tag: un pacchetto viene sostituito solo
da un candidato di un repository di priorità maggiore o uguale, quindi i
pacchetti SBo/locali/da sorgente non vengono mai migrati silenziosamente a un
altro repository né retrocessi. Le nuove dipendenze necessarie vengono mostrate
prima della conferma come `new-dep: [repo] pkg (for parent)`.

Installa i pacchetti aggiunti alla distribuzione dall'ultimo aggiornamento:

```
slacker install-new             # solo repository ufficiali (predefinito)
slacker install-new conraid     # solo i pacchetti appena aggiunti di conraid
slacker install-new slackware extras
```

## 6. Reinstallazione e rimozione

```
slacker reinstall bash          # reinstalla la versione corrente
slacker reinstall y             # reinstalla un'intera serie (qui: games)
slacker reinstall ap            # la serie 'ap'
slacker remove libfoo           # rimuove pacchetti installati
slacker --dry-run remove libfoo # anteprima
```

I nomi delle serie (`a`, `ap`, `d`, `k`, `kde`, `l`, `n`, `t`, `x`, `xap`,
`xfce`, `y`, ...) corrispondono esattamente a quella serie, non a ogni pacchetto
il cui nome contiene quelle lettere. Una corrispondenza multipla mostra comunque
l'elenco numerato di selezione.

## 7. Repository interi e build tag (selettori `@`)

Il prefisso `@` è un selettore di insieme esplicito. È obbligatorio — una parola
semplice non è mai trattata come repository.

```
slacker install @gnome          # installa ogni pacchetto del repository gnome
slacker remove  @gnome          # rimuove i pacchetti installati provenienti da quel repository
slacker remove  @_SBo           # rimuove tutti i pacchetti SlackBuilds.org installati
slacker download @alienbob      # scarica ogni pacchetto del repository alienbob
```

`@repo` significa "ogni pacchetto in quel repository"; `@_tag` significa "ogni
pacchetto con quel build tag". Un refuso dà un errore utile:

```
slacker install @gnme
# error: unknown repo or tag '@gnme'; did you mean '@gnome'?
#   available repos: conraid, gnome, slackware
#   available tags:  _gnome, cf
```

Uso tipico: metti un repository desktop (es. gnome) a una priorità alta e
distinta come 101 e installalo come insieme. I suoi pacchetti con tag `_gnome`
sono allora bloccati — nessun repository inferiore può sostituirli o
"aggiornarli", nemmeno con una versione più recente:

```
# /etc/slacker/repos
101  gnome  https://your-gnome-repo/...
```
```
slacker update
slacker install @gnome
slacker upgrade-all             # lascia i pacchetti gnome intatti
```

## 8. Gestione dei repository (aggiungere, rimuovere, tag, fiducia)

Puoi modificare `/etc/slacker/repos` a mano, oppure lasciare che lo faccia
slacker per te (con validazione e prompt di conferma):

```
slacker add-repo 70 extras https://.../slackware64-current/extra subtree
slacker add-repo 80 conraid https://slackers.it/repository/slackware64-current
slacker del-repo conraid
slacker add-tag 100 SBo _SBo            # una riga di priorità per build-tag
slacker del-tag _SBo
```

Flag di `add-repo` (qualsiasi ordine): `official`, `immutable`, `subtree`,
`verify=...`. Un **subtree** Slackware (`extra/`, `patches/`, `testing/`,
`pasture/`) deve avere `subtree` o i suoi pacchetti non si scaricano; `immutable`
tiene i pacchetti di un repository fuori da `clean-system` (vedi §12).

Ispeziona e controlla la salute in qualsiasi momento:

```
slacker list-repos              # priorità, conteggio installati, politica verify, flag
slacker status                  # controllo di salute dell'intera configurazione + cosa correggere
```

`list-repos` mostra una tabella e segna `(official)`, `(immutable)`, `(subtree)`
e ogni quarantena. `status` raggruppa i risultati (Setup / Installed / Online)
con indicatori ✓/!/✗ e termina con un verdetto in linguaggio semplice e i passi
successivi.

### Sicurezza dei repository: quarantena e fiducia

slacker controlla (vets) i repository e mette in **quarantena** quelli
irraggiungibili o che servono metadati malformati/ostili; un repository in
quarantena non fornisce alcun pacchetto finché non agisci. I repository nuovi o
non ancora fidati vengono controllati in modo leggero a ogni `update`; `add-repo`
e `vet-repo` eseguono un controllo approfondito.

```
slacker vet-repo conraid        # ri-controlla su richiesta (quarantena se fallisce, libera se passa)
slacker trust-repo conraid      # togli una quarantena che ritieni un falso positivo (override)
slacker distrust-repo conraid   # congela tu stesso un repository
```

Le chiavi GPG vengono fissate alla prima importazione (trust on first use): se la
chiave di un repository cambia, slacker la rifiuta come possibile attacco di
sostituzione della chiave invece di fidarsi silenziosamente della nuova. `list-repos`
e `status` mostrano lo stato.

## 9. Scaricare senza installare

I file vengono salvati in `CACHE_DIR/packages/<repo>/` per impostazione
predefinita, lo stesso posto in cui guarda `install`, così che
un'installazione successiva li riutilizzi.

```
slacker download pandoc-bin             # nella cache
slacker download -o /tmp pandoc-bin     # in /tmp invece
slacker download -o . pandoc-bin        # nella directory corrente
slacker download @alienbob              # repository intero (chiede conferma se >10)
```

slacker rifiuta di scrivere attraverso un symlink preesistente, quindi scaricare
in una directory condivisa come `/tmp` è sicuro.

## 10. Congelare i pacchetti (blacklist)

Congela un pacchetto affinché update, upgrade-all, reinstall e clean-system lo
lascino in pace (viene aggiunto a `/etc/slacker/blacklist`):

```
slacker frozen pandoc-bin               # congela uno
slacker frozen firefox chromium vlc     # congela diversi
slacker frozen "@alienbob vlc-*"        # limita a un repo + un pattern (virgolette obbligatorie)
```

Metti tra virgolette ogni regola che contiene uno spazio (una regola `@repo`) o un
carattere glob della shell (`*`, `?`, `[`, `]`, ...). `"@alienbob vlc-*"` limita la
regola al repo `alienbob`, e `vlc-*` viene confrontato come **regex non ancorata**
sull'id completo — in una regex `-*` significa "zero o più trattini", non
"qualsiasi cosa", quindi congela ogni pacchetto `alienbob` installato il cui id
contiene `vlc` (es. `vlc`, `vlc-plugin-qt`), proprio come farebbe un semplice
`vlc`. Per congelare solo il pacchetto `vlc`, ancorala: `"@alienbob ^vlc-[0-9]"`.

Usa il nome esatto del pacchetto (non il version-tag completo). Per scongelare,
rimuovi la riga da `/etc/slacker/blacklist`.

La blacklist congela singoli **pacchetti**. Per agire su un intero **repository**
c'è un meccanismo separato — la *quarantena*: `distrust-repo` congela un repo,
`vet-repo` lo ri-controlla, `trust-repo` lo libera (§8).

La blacklist è il modo per-pacchetto di tenere qualcosa fuori da `clean-system`.
Per proteggere un intero gruppo in una volta, preferisci `IGNORE_TAGS` (per build
tag, es. `_SBo cf alien`) o marcare un repository `immutable` (§8, §12) invece di
congelare ogni pacchetto a mano.

## 11. Template

Un template è un'istantanea dei nomi dei pacchetti installati che puoi
riprodurre su un'altra macchina o dopo una reinstallazione.

```
slacker generate-template mybox         # istantanea dei pacchetti correnti -> mybox.template
slacker install-template mybox          # installa tutto ciò che il template elenca
slacker remove-template mybox           # DISINSTALLA ogni pacchetto elencato dal template
slacker delete-template mybox           # elimina solo il file del template (mantiene i pacchetti)
```

Nota la distinzione: `remove-template` rimuove i *pacchetti*; `delete-template`
rimuove solo il *file*.

## 12. Pulizia

```
slacker clean-system            # elenca i pacchetti non più nella baseline ufficiale, scegli cosa rimuovere
slacker --dry-run clean-system  # anteprima prima — fallo sempre
slacker clean-cache             # elimina i *.txz scaricati dalla cache
slacker clean-cache alienbob    # solo i file in cache di quel repository
slacker --dry-run clean-cache   # mostra cosa verrebbe liberato
slacker new-config              # gestisce i file di configurazione *.new rimasti
```

`clean-cache` non tocca mai i metadati dei repository né le chiavi GPG (che
stanno sotto `CACHE_DIR/repos`), quindi è sempre sicuro eseguirlo.

`clean-system` è in stile slackpkg: rimuove i pacchetti che **non fanno più parte
della baseline ufficiale** — il `PACKAGES.TXT` del repository ufficiale più ogni
repository marcato `immutable`. Così un pacchetto che la distribuzione stessa ha
rimosso viene rimosso anche se un repository di terze parti fornisce ancora il
nome. Un pacchetto viene mantenuto (mai elencato) quando è vera una di tre cose:

- corrisponde a una regola di **blacklist** (`slacker frozen NAME`);
- il suo **build tag** è in `IGNORE_TAGS` (`slacker.conf`), es. `_SBo cf alien`;
- è attribuito a un repository **`immutable`** (il repository che possiede il suo
  build tag, o per un pacchetto senza tag qualsiasi repository immutable che ne
  fornisce il nome).

Quindi prima del tuo primo `clean-system`, imposta `IGNORE_TAGS` per i tuoi tag
SBo/locali/da-sorgente e/o marca i repository `extra/`/`testing/`/`patches/` come
`immutable` — altrimenti quei pacchetti compariranno come estranei. Come
salvaguardia `clean-system` rifiuta di eseguirsi se un repository della baseline
non ha metadati caricati (esegui prima `update`), e `--dry-run` mostra esattamente
cosa rimuoverebbe senza toccare nulla.

## 13. Flag globali e codici di uscita

I comandi di sola lettura (`search`, `info`, `file-search`, `check-updates`,
`show-changelog`) girano come qualsiasi utente. Tutto ciò che cambia il sistema,
la cache o la configurazione deve essere eseguito come root (o via sudo da un
membro di wheel); un tentativo non-root si ferma subito con un messaggio chiaro.

Questi comandi prendono anche un lock esclusivo (`/run/slacker.lock`) così che
due non possano girare insieme; una seconda invocazione esce subito riportando il
PID in esecuzione. Il lock viene rilasciato automaticamente se slacker termina o
viene ucciso, così un crash non ti blocca mai fuori. Le interrogazioni non
prendono lock.

Flag (funzionano con ogni comando):

```
--config-dir <DIR>    usa una directory di configurazione diversa (predefinita /etc/slacker)
-y, --yes             assume "sì" a tutti i prompt
--dry-run             mostra cosa accadrebbe, non cambia nulla
--no-deps             non leggere i file .dep per questa esecuzione
```

Codici di uscita:

```
0     successo
1     errore
20    niente trovato / niente da fare
50    è disponibile un auto-aggiornamento di slacker
100   aggiornamenti in sospeso (da check-updates)
```

Esempio di controllo in uno script:

```
slacker check-updates ; [ $? -eq 100 ] && slacker -y upgrade-all
```

## 14. Flussi di lavoro comuni

Aggiornamento di routine del sistema:

```
slacker update
slacker upgrade-all
slacker --dry-run clean-system   # controlla prima (rimuove tutto ciò che è fuori dalla baseline)
slacker clean-system             # poi eseguilo, una volta impostati IGNORE_TAGS/immutable (vedi §12)
```

Prima sincronizzazione dopo aver modificato i repos, con importazione chiavi:

```
slacker update gpg
slacker update
slacker check-updates ; echo "exit=$?"
```

Anteprima di tutto prima di impegnarsi:

```
slacker --dry-run upgrade-all
slacker --dry-run install @gnome
slacker --dry-run clean-cache
```

Spostare l'insieme di pacchetti di una macchina su un'altra:

```
# sulla macchina di origine
slacker generate-template snapshot
# copia /etc/slacker/templates/snapshot.template sul target, poi:
slacker update
slacker install-template snapshot
```

Liberare spazio senza rischiare metadati o chiavi:

```
slacker clean-cache
```

## 15. Verifica dei pacchetti

slacker verifica i pacchetti prima di installarli. La politica si imposta
globalmente con `VERIFY` in `slacker.conf` e si può sovrascrivere per repository
con un flag `verify=` sulla riga del repo.

Predefinita (`VERIFY=all`):

```
# slacker.conf
VERIFY=all
```

Con `all`, per ogni pacchetto: la firma GPG viene controllata quando il
repository ne fornisce una (una firma errata fallisce sempre; una mancante viene
saltata), e almeno un checksum di integrità (md5 o sha) deve essere presente e
corrispondere. Se né md5 né sha sono disponibili, l'installazione si ferma — il
file checksum del repository manca o è rotto.

Slackware fornisce un `.txz.asc` per pacchetto accanto a ogni pacchetto, quindi
con `all` slacker verifica via GPG il pacchetto stesso e stampa, es.,
`verified: gpg (signer) + md5`. Per questo devi aver fissato la chiave del
repository con `slacker update gpg`; fino ad allora ottieni `integrity only: md5`
(il pacchetto è comunque verificato con md5 rispetto al `CHECKSUMS.md5` firmato
con GPG, solo non autenticato per pacchetto). Per un repository `subtree` la
chiave viene presa dalla radice, dove Slackware tiene l'unica chiave che firma
l'intero albero — così `extra/`/`testing/`/`patches/` fissano lo stesso
fingerprint del repository ufficiale.

**Pinning della chiave (trust on first use):** la prima importazione fissa il
fingerprint del repository; se mai cambia, slacker rifiuta il repository come
possibile attacco di sostituzione della chiave invece di fidarsi silenziosamente
della nuova. Vedi §8 per i comandi di quarantena/fiducia.

Richiedi metodi specifici (si ferma se ne manca uno, dicendoti come allentarlo):

```
VERIFY=gpg,md5,sha
VERIFY=gpg,md5
VERIFY=md5
```

Disabilita completamente (sconsigliato):

```
VERIFY=none
```

Override per repository — utile quando un repository ha un checksum o una firma
rotti o mancanti, così allenti solo quello invece di indebolire tutto:

```
# repos
100  slackware  mirror                       official
80   conraid    https://slackers.it/...      verify=gpg,md5
60   alienbob   https://slackware.nl/...      verify=md5
```

Le stesse regole valgono per ogni repository, incluso quello ufficiale — non c'è
esenzione. Il flag `official` influisce solo sull'ambito di `install-new` e sul
tracciamento del ChangeLog, non sulla verifica.

Se uno scaricamento fallisce la verifica vedrai un messaggio chiaro, per esempio:

```
md5 mismatch for foo-1.0-x86_64-1cf.txz: expected ..., got ...
no usable checksum (md5 or sha) for foo-...: the repo's checksum file may be
  missing or broken. ... relax verification for it with a `verify=` flag ...
```

---

<a id="ru"></a>
# Русский

Практическое руководство по `slacker` с примерами — двоичному менеджеру пакетов
для Slackware, объединяющему `slackpkg` и `slackpkg+` в одном инструменте.

Каждый пример ниже — реальная команда. Действия, изменяющие систему (install,
upgrade, remove, ...), требуют root; запросы (search, info, file-search, ...) —
нет.

## Содержание

1. Первичная настройка
2. Поддержание метаданных свежими
3. Поиск, осмотр и список репозиториев
4. Установка
5. Обновление
6. Переустановка и удаление
7. Целые репозитории и build-теги (селекторы `@`)
8. Управление репозиториями (добавить, удалить, теги, доверие)
9. Загрузка без установки
10. «Заморозка» пакетов (blacklist)
11. Шаблоны (templates)
12. Очистка
13. Глобальные флаги и коды возврата
14. Типичные сценарии
15. Проверка пакетов

## 1. Первичная настройка

Конфигурация находится в `/etc/slacker/` (переопределяется через `--config-dir`).
Скопируйте поставляемые шаблоны и отредактируйте их:

```


```

Выберите ровно одно зеркало в `/etc/slacker/mirrors` (по умолчанию ни одно не
активно; две и более активные строки — ошибка):

```
# /etc/slacker/mirrors  - uncomment ONE line
https://slackware.uk/slackware/slackware64-current/
```

Объявите репозитории в `/etc/slacker/repos`. Двоичные репозитории принимают URL
(или ключевое слово `mirror` для официального) и должны иметь **различные**
приоритеты; побеждает наивысший:

```
# priority  name        url|mirror                                            [flags]
100         slackware   mirror                                                official
90          extras      https://mirror.nl.leaseweb.net/slackware/slackware64-current/extra   subtree
80          conraid     https://slackers.it/repository/slackware64-current
60          alienbob    https://slackware.nl/people/alien/sbrepos/current/x86_64
```

Флаги (в любом порядке) идут после URL: `official` (отслеживаемый репозиторий),
`subtree` (поддерево дистрибутива Slackware — см. ниже), `immutable` (репозиторий,
пакеты которого `clean-system` никогда не удаляет) и `verify=...`
(переопределение проверки для репозитория).

Четыре поддерева дистрибутива Slackware — **`extra`, `patches`, `testing`,
`pasture`** — **обязаны всегда нести флаг `subtree`** (в любой позиции после URL).
Их `PACKAGES.TXT` указывает расположения пакетов относительно корня дистрибутива,
поэтому без `subtree` их пакеты не загружаются (удвоенный сегмент пути); с ним
пакеты и `GPG-KEY` берутся из родительского (корневого) URL, тогда как метаданные
по-прежнему берутся из URL самого репозитория. Для этих четырёх репозиториев это
не опционально.

При желании добавьте строки приоритета тегов, чтобы пакеты из исходников/локальные
никогда не мигрировали и не понижались командой `upgrade-all`:

```
100         SBo         _SBo
100         local       _rtz
```

Один раз импортируйте GPG-ключи репозиториев, затем обновите и проверьте
настройку:

```
slacker update gpg
slacker update
slacker status                  # подтверждает, что настройка исправна, и отмечает, что нужно поправить
```

`update gpg` закрепляет (pin) ключ каждого репозитория (trust on first use). Для
репозитория `subtree` он закрепляет тот же ключ Slackware из корня — именно это
позволяет пройти per-package GPG-проверке для пакетов
`extra/`/`testing/`/`patches/` при установке.

## 2. Поддержание метаданных свежими

```
slacker update                  # обновить PACKAGES.TXT/CHECKSUMS, проверить GPG
slacker update gpg              # (пере)импортировать GPG-ключи, затем обновить
slacker check-updates           # проверить каждый репозиторий; код 100, если есть ожидающие обновления
slacker show-changelog          # показать кешированный ChangeLog (с пейджером в TTY)
slacker show-changelog conraid  # ChangeLog указанного репозитория (загружается по запросу)
```

## 3. Поиск, осмотр и список репозиториев

```
slacker search firefox          # найти пакет по точному имени (без учёта регистра)
slacker info bash               # кандидаты по репозиториям + установленная версия
slacker file-search bin/bash    # какой пакет поставляет этот файл (использует MANIFEST)
slacker list-repos              # репозитории: приоритет, число установленных, verify, флаги
slacker status                  # проверка здоровья всей настройки; подсказывает, что исправить
```

`info` показывает, какой репозиторий побеждает по приоритету. Например, если
`ffmpeg` есть в нескольких репозиториях, кандидатом будет наивысший по приоритету;
закрепите другой через `repo:name` (см. ниже).

## 4. Установка

```
slacker install vlc                     # один пакет (+ его цепочку .dep)
slacker install vlc mpv obs-studio      # несколько сразу
slacker install conraid:ffmpeg          # принудительно сборку conraid (pin)
slacker --dry-run install vlc           # только предпросмотр, ничего не меняет
slacker --no-deps install vlc           # пропустить разрешение зависимостей
slacker -y install vlc                  # отвечать «да» на все запросы
```

Если шаблон совпадает с более чем одним пакетом, slacker печатает нумерованный
список:

```
slacker install python
# 'install' matched 12 packages:
#   1) [slackware] python-build-...
#   2) [slackware] python-cffi-...
#   ...
# Enter numbers to install (e.g. 1 3 5 or 2-4), Enter for all, 'n' to cancel:
```

Уже установленные пакеты `install` отклоняет (используйте `upgrade` или
`reinstall`).

**Зависимости — функция сторонних репозиториев, а не официальная.** Собственные
репозитории Slackware — `slackware` и поддеревья
`extra`/`patches`/`testing`/`pasture` — не поставляют и не ожидают информации о
зависимостях: официальное требование — полная установка **всех** наборов пакетов
Slackware, поэтому каждая зависимость считается уже присутствующей, и slacker не
выполняет для них никакого разрешения зависимостей (оно не нужно). Независимые
репозитории, такие как `alienbob` и `conraid`, поставляют файлы `.dep` для каждого
пакета; slacker читает их **только для репозиториев, которые их предоставляют**,
подтягивая недостающие зависимости из того же репозитория. Поэтому
`install`/`upgrade` из официального дерева ничего не разрешает, а из стороннего
репозитория с файлами `.dep` — разрешает. Отключите везде через `--no-deps` (на
запуск) или `RESOLVE_DEPS=no` (в `slacker.conf`).

## 5. Обновление

```
slacker upgrade vlc             # обновить конкретные пакеты
slacker upgrade-all             # обновить всё, у чего есть более новая ревизия
slacker --dry-run upgrade-all   # сначала предпросмотр всего плана обновления
slacker -y upgrade-all          # без запросов
```

`upgrade-all` уважает приоритет и build-теги: пакет заменяется только кандидатом
из репозитория с большим или равным приоритетом, поэтому пакеты
SBo/локальные/из-исходников никогда не мигрируют молча в другой репозиторий и не
понижаются. Нужные новые зависимости показываются перед подтверждением как
`new-dep: [repo] pkg (for parent)`.

Установить пакеты, добавленные в дистрибутив с момента вашего последнего
обновления:

```
slacker install-new             # только официальные репозитории (по умолчанию)
slacker install-new conraid     # только недавно добавленные пакеты conraid
slacker install-new slackware extras
```

## 6. Переустановка и удаление

```
slacker reinstall bash          # переустановить текущую версию
slacker reinstall y             # переустановить целую серию (здесь: games)
slacker reinstall ap            # серия 'ap'
slacker remove libfoo           # удалить установленные пакеты
slacker --dry-run remove libfoo # предпросмотр
```

Имена серий (`a`, `ap`, `d`, `k`, `kde`, `l`, `n`, `t`, `x`, `xap`, `xfce`, `y`,
...) совпадают именно с этой серией, а не с каждым пакетом, в имени которого есть
эти буквы. При множественном совпадении всё равно показывается нумерованный список
выбора.

## 7. Целые репозитории и build-теги (селекторы `@`)

Префикс `@` — явный селектор множества. Он обязателен — простое слово никогда не
считается репозиторием.

```
slacker install @gnome          # установить каждый пакет репозитория gnome
slacker remove  @gnome          # удалить установленные пакеты из этого репозитория
slacker remove  @_SBo           # удалить все установленные пакеты SlackBuilds.org
slacker download @alienbob      # загрузить каждый пакет репозитория alienbob
```

`@repo` означает «каждый пакет в этом репозитории»; `@_tag` означает «каждый
пакет с этим build-тегом». Опечатка даёт полезную ошибку:

```
slacker install @gnme
# error: unknown repo or tag '@gnme'; did you mean '@gnome'?
#   available repos: conraid, gnome, slackware
#   available tags:  _gnome, cf
```

Типичное применение: поместите репозиторий рабочего стола (например gnome) на
отдельный высокий приоритет, например 101, и установите его как набор. Тогда его
пакеты с тегом `_gnome` заблокированы — никакой репозиторий ниже не сможет
заменить или «обновить» их даже более новой версией:

```
# /etc/slacker/repos
101  gnome  https://your-gnome-repo/...
```
```
slacker update
slacker install @gnome
slacker upgrade-all             # оставляет пакеты gnome нетронутыми
```

## 8. Управление репозиториями (добавить, удалить, теги, доверие)

Вы можете править `/etc/slacker/repos` вручную или поручить это slacker (с
валидацией и запросом подтверждения):

```
slacker add-repo 70 extras https://.../slackware64-current/extra subtree
slacker add-repo 80 conraid https://slackers.it/repository/slackware64-current
slacker del-repo conraid
slacker add-tag 100 SBo _SBo            # строка приоритета build-тега
slacker del-tag _SBo
```

Флаги `add-repo` (любой порядок): `official`, `immutable`, `subtree`,
`verify=...`. Поддерево Slackware (`extra/`, `patches/`, `testing/`, `pasture/`)
обязано получить `subtree`, иначе его пакеты не загрузятся; `immutable` держит
пакеты репозитория вне `clean-system` (см. §12).

Осматривайте и проверяйте здоровье в любой момент:

```
slacker list-repos              # приоритет, число установленных, политика verify, флаги
slacker status                  # проверка здоровья всей настройки + что исправить
```

`list-repos` показывает таблицу и помечает `(official)`, `(immutable)`,
`(subtree)` и любой карантин. `status` группирует находки (Setup / Installed /
Online) с маркерами ✓/!/✗ и завершает понятным вердиктом и следующими шагами.

### Безопасность репозиториев: карантин и доверие

slacker проверяет (vets) репозитории и помещает в **карантин** недоступные или
отдающие искажённые/враждебные метаданные; репозиторий в карантине не
предоставляет пакетов, пока вы не вмешаетесь. Новые или ещё не доверенные
репозитории легко проверяются при каждом `update`; `add-repo` и `vet-repo`
проверяют тщательно.

```
slacker vet-repo conraid        # перепроверить по запросу (карантин при провале, снятие при успехе)
slacker trust-repo conraid      # снять карантин, который вы считаете ложным срабатыванием (override)
slacker distrust-repo conraid   # заморозить репозиторий самостоятельно
```

GPG-ключи закрепляются при первом импорте (trust on first use): если ключ
репозитория когда-либо изменится, slacker отклонит его как возможную атаку
подмены ключа, а не доверится новому молча. `list-repos` и `status` показывают
состояние.

## 9. Загрузка без установки

Файлы по умолчанию сохраняются в `CACHE_DIR/packages/<repo>/` — туда же смотрит
`install`, поэтому последующая установка использует их повторно.

```
slacker download pandoc-bin             # в кеш
slacker download -o /tmp pandoc-bin     # в /tmp вместо этого
slacker download -o . pandoc-bin        # в текущий каталог
slacker download @alienbob              # весь репозиторий (спросит подтверждение, если >10)
```

slacker отказывается писать сквозь существующий симлинк, поэтому загрузка в общий
каталог вроде `/tmp` безопасна.

## 10. «Заморозка» пакетов (blacklist)

Заморозьте пакет, чтобы update, upgrade-all, reinstall и clean-system его не
трогали (он добавляется в `/etc/slacker/blacklist`):

```
slacker frozen pandoc-bin               # заморозить один
slacker frozen firefox chromium vlc     # заморозить несколько
slacker frozen "@alienbob vlc-*"        # ограничить репозиторием + шаблон (кавычки обязательны)
```

Заключайте в кавычки любое правило с пробелом (правило `@repo`) или символом
shell-glob (`*`, `?`, `[`, `]`, ...). `"@alienbob vlc-*"` ограничивает правило
репозиторием `alienbob`, а `vlc-*` сопоставляется как **незаякоренное регулярное
выражение** с полным id — в регулярном выражении `-*` означает «ноль или более
дефисов», а не «что угодно», поэтому оно замораживает любой установленный пакет
`alienbob`, чей id содержит `vlc` (напр. `vlc`, `vlc-plugin-qt`), ровно как и
простое `vlc`. Чтобы заморозить только пакет `vlc`, заякорите:
`"@alienbob ^vlc-[0-9]"`.

Используйте точное имя пакета (не полный version-tag). Чтобы разморозить, удалите
строку из `/etc/slacker/blacklist`.

Blacklist замораживает отдельные **пакеты**. Чтобы воздействовать на целый
**репозиторий**, есть отдельный механизм — *карантин*: `distrust-repo` замораживает
репозиторий, `vet-repo` перепроверяет его, `trust-repo` снимает карантин (§8).

Blacklist — это per-package способ держать что-то вне `clean-system`. Чтобы
защитить целую группу сразу, предпочтите `IGNORE_TAGS` (по build-тегу, напр.
`_SBo cf alien`) или пометку репозитория `immutable` (§8, §12) вместо ручной
заморозки каждого пакета.

## 11. Шаблоны (templates)

Шаблон — это снимок имён установленных пакетов, который можно воспроизвести на
другой машине или после переустановки.

```
slacker generate-template mybox         # снимок текущих пакетов -> mybox.template
slacker install-template mybox          # установить всё, что перечисляет шаблон
slacker remove-template mybox           # УДАЛИТЬ каждый пакет, перечисленный в шаблоне
slacker delete-template mybox           # удалить только файл шаблона (пакеты остаются)
```

Обратите внимание на различие: `remove-template` удаляет *пакеты*;
`delete-template` удаляет только *файл*.

## 12. Очистка

```
slacker clean-system            # список пакетов, которых уже нет в официальной baseline, выбрать что удалить
slacker --dry-run clean-system  # сначала предпросмотр — делайте это всегда
slacker clean-cache             # удалить скачанные *.txz из кеша
slacker clean-cache alienbob    # только кешированные файлы этого репозитория
slacker --dry-run clean-cache   # показать, что будет освобождено
slacker new-config              # обработать оставшиеся файлы конфигурации *.new
```

`clean-cache` никогда не трогает метаданные репозиториев или GPG-ключи (они лежат
под `CACHE_DIR/repos`), поэтому запускать его всегда безопасно.

`clean-system` в стиле slackpkg: он удаляет пакеты, которые **больше не входят в
официальную baseline** — `PACKAGES.TXT` официального репозитория плюс любой
репозиторий, помеченный `immutable`. Так пакет, который сам дистрибутив убрал,
удаляется, даже если сторонний репозиторий всё ещё отдаёт это имя. Пакет
сохраняется (никогда не попадает в список), когда верно одно из трёх:

- он совпадает с правилом **blacklist** (`slacker frozen NAME`);
- его **build-тег** есть в `IGNORE_TAGS` (`slacker.conf`), напр. `_SBo cf alien`;
- он отнесён к **`immutable`**-репозиторию (репозиторий, владеющий его
  build-тегом, или для пакета без тега — любой immutable-репозиторий, дающий его
  имя).

Поэтому перед первым `clean-system` задайте `IGNORE_TAGS` для своих
SBo/локальных/из-исходников тегов и/или пометьте репозитории
`extra/`/`testing/`/`patches/` как `immutable` — иначе эти пакеты будут показаны
как чужие. В качестве предохранителя `clean-system` отказывается работать, если у
baseline-репозитория не загружены метаданные (сначала выполните `update`), а
`--dry-run` показывает ровно то, что было бы удалено, ничего не трогая.

## 13. Глобальные флаги и коды возврата

Команды только для чтения (`search`, `info`, `file-search`, `check-updates`,
`show-changelog`) выполняются от любого пользователя. Всё, что меняет систему,
кеш или конфигурацию, должно выполняться от root (или через sudo членом группы
wheel); попытка не-root сразу останавливается с понятным сообщением.

Эти команды также берут эксклюзивную блокировку (`/run/slacker.lock`), чтобы две
не работали одновременно; второй запуск сразу выходит, сообщая PID работающего.
Блокировка снимается автоматически, если slacker завершается или убит, поэтому
крах никогда не запирает вас. Запросы блокировку не берут.

Флаги (работают с любой командой):

```
--config-dir <DIR>    использовать другой каталог конфигурации (по умолчанию /etc/slacker)
-y, --yes             отвечать «да» на все запросы
--dry-run             показать, что произошло бы, ничего не меняя
--no-deps             не читать файлы .dep в этом запуске
```

Коды возврата:

```
0     успех
1     ошибка
20    ничего не найдено / нечего делать
50    доступно самообновление slacker
100   ожидающие обновления (от check-updates)
```

Пример проверки в скрипте:

```
slacker check-updates ; [ $? -eq 100 ] && slacker -y upgrade-all
```

## 14. Типичные сценарии

Плановое обновление системы:

```
slacker update
slacker upgrade-all
slacker --dry-run clean-system   # сначала просмотрите (удаляет всё, что вне baseline)
slacker clean-system             # затем запустите, задав IGNORE_TAGS/immutable (см. §12)
```

Первая синхронизация после правки repos, с импортом ключей:

```
slacker update gpg
slacker update
slacker check-updates ; echo "exit=$?"
```

Предпросмотр всего перед применением:

```
slacker --dry-run upgrade-all
slacker --dry-run install @gnome
slacker --dry-run clean-cache
```

Перенести набор пакетов одной машины на другую:

```
# на исходной машине
slacker generate-template snapshot
# скопируйте /etc/slacker/templates/snapshot.template на цель, затем:
slacker update
slacker install-template snapshot
```

Освободить место, не рискуя метаданными или ключами:

```
slacker clean-cache
```

## 15. Проверка пакетов

slacker проверяет пакеты перед установкой. Политика задаётся глобально через
`VERIFY` в `slacker.conf` и может быть переопределена для репозитория флагом
`verify=` в строке repo.

По умолчанию (`VERIFY=all`):

```
# slacker.conf
VERIFY=all
```

С `all`, для каждого пакета: GPG-подпись проверяется, когда репозиторий её
предоставляет (плохая подпись всегда проваливается; отсутствующая пропускается),
и хотя бы одна контрольная сумма целостности (md5 или sha) должна присутствовать
и совпадать. Если нет ни md5, ни sha, установка останавливается — файл
контрольных сумм репозитория отсутствует или повреждён.

Slackware кладёт per-package `.txz.asc` рядом с каждым пакетом, поэтому при `all`
slacker GPG-проверяет сам пакет и печатает, напр., `verified: gpg (signer) + md5`.
Для этого нужно закрепить ключ репозитория через `slacker update gpg`; до этого вы
получаете `integrity only: md5` (пакет всё равно проверяется по md5 относительно
подписанного GPG `CHECKSUMS.md5`, просто не аутентифицируется per package). Для
репозитория `subtree` ключ берётся из корня, где Slackware держит единственный
ключ, подписывающий всё дерево, — поэтому `extra/`/`testing/`/`patches/`
закрепляют тот же fingerprint, что и официальный репозиторий.

**Закрепление ключа (trust on first use):** первый импорт закрепляет fingerprint
репозитория; если он когда-нибудь изменится, slacker отклонит репозиторий как
возможную атаку подмены ключа, а не доверится новому молча. См. §8 о командах
карантина/доверия.

Требовать конкретные методы (останавливается, если одного нет, подсказывая, как
ослабить):

```
VERIFY=gpg,md5,sha
VERIFY=gpg,md5
VERIFY=md5
```

Полностью отключить (не рекомендуется):

```
VERIFY=none
```

Переопределение для репозитория — полезно, когда у одного репозитория сломаны или
отсутствуют контрольная сумма или подпись, чтобы ослабить только его, а не всё:

```
# repos
100  slackware  mirror                       official
80   conraid    https://slackers.it/...      verify=gpg,md5
60   alienbob   https://slackware.nl/...      verify=md5
```

Те же правила действуют для каждого репозитория, включая официальный, — без
исключений. Флаг `official` влияет только на охват `install-new` и отслеживание
ChangeLog, но не на проверку.

Если загрузка не проходит проверку, вы увидите понятное сообщение, например:

```
md5 mismatch for foo-1.0-x86_64-1cf.txz: expected ..., got ...
no usable checksum (md5 or sha) for foo-...: the repo's checksum file may be
  missing or broken. ... relax verification for it with a `verify=` flag ...
```

---

<a id="es"></a>
# Español

Una guía práctica y basada en ejemplos de `slacker`, un gestor de paquetes
binarios para Slackware que combina `slackpkg` y `slackpkg+` en una sola
herramienta.

Cada ejemplo a continuación es un comando real. Las acciones que modifican el
sistema (install, upgrade, remove, ...) necesitan root; las consultas (search,
info, file-search, ...) no.

## Índice

1. Configuración inicial
2. Mantener los metadatos al día
3. Búsqueda, inspección y listado de repositorios
4. Instalación
5. Actualización
6. Reinstalación y eliminación
7. Repositorios enteros y build tags (selectores `@`)
8. Gestión de repositorios (añadir, quitar, tags, confianza)
9. Descargar sin instalar
10. Congelar paquetes (blacklist)
11. Plantillas (templates)
12. Limpieza
13. Flags globales y códigos de salida
14. Flujos de trabajo comunes
15. Verificación de paquetes

## 1. Configuración inicial

La configuración está en `/etc/slacker/` (se sobrescribe con `--config-dir`).
Copia las plantillas incluidas y edítalas:

```


```

Elige exactamente un mirror en `/etc/slacker/mirrors` (ninguno está activo por
defecto; dos o más líneas activas es un error):

```
# /etc/slacker/mirrors  - uncomment ONE line
https://slackware.uk/slackware/slackware64-current/
```

Declara tus repositorios en `/etc/slacker/repos`. Los repositorios binarios toman
una URL (o la palabra clave `mirror` para el oficial) y deben tener prioridades
**distintas**; gana la más alta:

```
# priority  name        url|mirror                                            [flags]
100         slackware   mirror                                                official
90          extras      https://mirror.nl.leaseweb.net/slackware/slackware64-current/extra   subtree
80          conraid     https://slackers.it/repository/slackware64-current
60          alienbob    https://slackware.nl/people/alien/sbrepos/current/x86_64
```

Los flags (en cualquier orden) van después de la URL: `official` (el repositorio
rastreado), `subtree` (un subárbol de la distribución Slackware — ver abajo),
`immutable` (un repositorio cuyos paquetes `clean-system` nunca elimina) y
`verify=...` (anulación de la verificación por repositorio).

Los cuatro subárboles de la distribución Slackware — **`extra`, `patches`,
`testing`, `pasture`** — **deben llevar siempre el flag `subtree`** (en cualquier
posición tras la URL). Su `PACKAGES.TXT` lista las ubicaciones de los paquetes
relativas a la raíz de la distribución, así que sin `subtree` sus paquetes no se
descargan (un segmento de ruta duplicado); con él, los paquetes y `GPG-KEY` se
obtienen de la URL padre (raíz) mientras los metadatos siguen viniendo de la URL
del propio repositorio. Para estos cuatro repositorios no es opcional.

Opcionalmente, añade líneas de prioridad de tag para que los paquetes de
fuente/locales nunca sean migrados ni degradados por `upgrade-all`:

```
100         SBo         _SBo
100         local       _rtz
```

Importa una vez las claves GPG de los repositorios, luego actualiza y comprueba
la configuración:

```
slacker update gpg
slacker update
slacker status                  # confirma que la configuración está sana y señala qué corregir
```

`update gpg` fija (pin) la clave de cada repositorio (trust on first use). Para un
repositorio `subtree` fija la misma clave de Slackware desde la raíz, que es
justo lo que permite que la verificación GPG por paquete funcione para los
paquetes `extra/`/`testing/`/`patches/` al instalar.

## 2. Mantener los metadatos al día

```
slacker update                  # refresca PACKAGES.TXT/CHECKSUMS, verifica GPG
slacker update gpg              # (re)importa las claves GPG, luego refresca
slacker check-updates           # comprueba cada repositorio; sale con 100 si hay actualizaciones pendientes
slacker show-changelog          # muestra el ChangeLog en caché (con paginador en TTY)
slacker show-changelog conraid  # ChangeLog de un repositorio indicado (descargado bajo demanda)
```

## 3. Búsqueda, inspección y listado de repositorios

```
slacker search firefox          # encuentra un paquete por su nombre exacto (sin distinguir mayúsculas)
slacker info bash               # candidatos por repositorio + versión instalada
slacker file-search bin/bash    # qué paquete incluye este archivo (usa MANIFEST)
slacker list-repos              # repositorios: prioridad, número de instalados, verify, flags
slacker status                  # chequeo de salud de toda la configuración; dice qué corregir
```

`info` muestra qué repositorio gana por prioridad. Por ejemplo, si `ffmpeg`
existe en varios repositorios, el candidato es el de mayor prioridad; fija otro
con `repo:name` (ver abajo).

## 4. Instalación

```
slacker install vlc                     # un paquete (+ su cadena de .dep)
slacker install vlc mpv obs-studio      # varios a la vez
slacker install conraid:ffmpeg          # fuerza la build de conraid (pin)
slacker --dry-run install vlc           # solo vista previa, no cambia nada
slacker --no-deps install vlc           # omite la resolución de dependencias
slacker -y install vlc                  # asume "sí" a todos los prompts
```

Si un patrón coincide con más de un paquete, slacker imprime una lista numerada:

```
slacker install python
# 'install' matched 12 packages:
#   1) [slackware] python-build-...
#   2) [slackware] python-cffi-...
#   ...
# Enter numbers to install (e.g. 1 3 5 or 2-4), Enter for all, 'n' to cancel:
```

Los paquetes ya instalados son rechazados por `install` (usa `upgrade` o
`reinstall`).

**Las dependencias son una función de terceros, no oficial.** Los repositorios
propios de Slackware — `slackware` y los subárboles
`extra`/`patches`/`testing`/`pasture` — no proveen ni esperan información de
dependencias: una instalación completa de **todos** los conjuntos de paquetes de
Slackware es el requisito oficial, así que cada dependencia se asume ya presente,
y slacker no realiza ninguna resolución de dependencias para ellos (no hace
falta). Repositorios independientes como `alienbob` y `conraid` sí proveen
archivos `.dep` por paquete; slacker los lee **solo para los repositorios que los
proveen**, trayendo las dependencias faltantes desde ese mismo repositorio. Así,
un `install`/`upgrade` desde el árbol oficial no resuelve nada, mientras que uno
desde un repositorio de terceros con archivos `.dep` sí. Desactívalo en cualquier
parte con `--no-deps` (por ejecución) o `RESOLVE_DEPS=no` (en `slacker.conf`).

## 5. Actualización

```
slacker upgrade vlc             # actualiza paquetes concretos
slacker upgrade-all             # actualiza todo lo que tenga una revisión más nueva
slacker --dry-run upgrade-all   # vista previa de todo el plan de actualización primero
slacker -y upgrade-all          # sin prompts
```

`upgrade-all` respeta la prioridad y los build tags: un paquete solo se
reemplaza por un candidato de un repositorio de prioridad mayor o igual, así que
los paquetes SBo/locales/de fuente nunca se migran en silencio a otro repositorio
ni se degradan. Las nuevas dependencias que necesita se muestran antes de la
confirmación como `new-dep: [repo] pkg (for parent)`.

Instalar paquetes añadidos a la distribución desde tu última actualización:

```
slacker install-new             # solo repositorios oficiales (por defecto)
slacker install-new conraid     # solo los paquetes recién añadidos de conraid
slacker install-new slackware extras
```

## 6. Reinstalación y eliminación

```
slacker reinstall bash          # reinstala la versión actual
slacker reinstall y             # reinstala una serie entera (aquí: games)
slacker reinstall ap            # la serie 'ap'
slacker remove libfoo           # elimina paquetes instalados
slacker --dry-run remove libfoo # vista previa
```

Los nombres de serie (`a`, `ap`, `d`, `k`, `kde`, `l`, `n`, `t`, `x`, `xap`,
`xfce`, `y`, ...) coinciden exactamente con esa serie, no con cada paquete cuyo
nombre contenga esas letras. Una coincidencia múltiple muestra igualmente la
lista numerada de selección.

## 7. Repositorios enteros y build tags (selectores `@`)

El prefijo `@` es un selector de conjunto explícito. Es obligatorio — una palabra
suelta nunca se trata como repositorio.

```
slacker install @gnome          # instala cada paquete del repositorio gnome
slacker remove  @gnome          # elimina los paquetes instalados procedentes de ese repositorio
slacker remove  @_SBo           # elimina todos los paquetes SlackBuilds.org instalados
slacker download @alienbob      # descarga cada paquete del repositorio alienbob
```

`@repo` significa "cada paquete de ese repositorio"; `@_tag` significa "cada
paquete con ese build tag". Un error tipográfico da un mensaje útil:

```
slacker install @gnme
# error: unknown repo or tag '@gnme'; did you mean '@gnome'?
#   available repos: conraid, gnome, slackware
#   available tags:  _gnome, cf
```

Uso típico: pon un repositorio de escritorio (p. ej. gnome) en una prioridad alta
y distinta como 101 e instálalo como conjunto. Sus paquetes con tag `_gnome`
quedan entonces bloqueados — ningún repositorio inferior puede reemplazarlos ni
"actualizarlos", ni siquiera con una versión más nueva:

```
# /etc/slacker/repos
101  gnome  https://your-gnome-repo/...
```
```
slacker update
slacker install @gnome
slacker upgrade-all             # deja los paquetes de gnome intactos
```

## 8. Gestión de repositorios (añadir, quitar, tags, confianza)

Puedes editar `/etc/slacker/repos` a mano, o dejar que slacker lo haga por ti
(con validación y prompt de confirmación):

```
slacker add-repo 70 extras https://.../slackware64-current/extra subtree
slacker add-repo 80 conraid https://slackers.it/repository/slackware64-current
slacker del-repo conraid
slacker add-tag 100 SBo _SBo            # una línea de prioridad de build-tag
slacker del-tag _SBo
```

Flags de `add-repo` (cualquier orden): `official`, `immutable`, `subtree`,
`verify=...`. Un **subtree** de Slackware (`extra/`, `patches/`, `testing/`,
`pasture/`) debe llevar `subtree` o sus paquetes no se descargan; `immutable`
mantiene los paquetes de un repositorio fuera de `clean-system` (ver §12).

Inspecciona y haz chequeo de salud en cualquier momento:

```
slacker list-repos              # prioridad, número de instalados, política verify, flags
slacker status                  # chequeo de salud de toda la configuración + qué corregir
```

`list-repos` muestra una tabla y marca `(official)`, `(immutable)`, `(subtree)` y
cualquier cuarentena. `status` agrupa sus hallazgos (Setup / Installed / Online)
con marcadores ✓/!/✗ y termina con un veredicto en lenguaje claro y los próximos
pasos.

### Seguridad de repositorios: cuarentena y confianza

slacker examina (vets) los repositorios y pone en **cuarentena** los que son
inalcanzables o sirven metadatos malformados/hostiles; un repositorio en
cuarentena no provee ningún paquete hasta que actúes. Los repositorios nuevos o
aún no confiables se examinan de forma ligera en cada `update`; `add-repo` y
`vet-repo` examinan a fondo.

```
slacker vet-repo conraid        # revisa de nuevo bajo demanda (cuarentena si falla, libera si pasa)
slacker trust-repo conraid      # levanta una cuarentena que juzgues un falso positivo (override)
slacker distrust-repo conraid   # congela tú mismo un repositorio
```

Las claves GPG se fijan en la primera importación (trust on first use): si la
clave de un repositorio cambia alguna vez, slacker lo rechaza como posible ataque
de sustitución de clave en lugar de confiar en la nueva en silencio. `list-repos`
y `status` muestran el estado.

## 9. Descargar sin instalar

Los archivos se guardan en `CACHE_DIR/packages/<repo>/` por defecto, el mismo
lugar donde mira `install`, de modo que una instalación posterior los reutilice.

```
slacker download pandoc-bin             # a la caché
slacker download -o /tmp pandoc-bin     # a /tmp en su lugar
slacker download -o . pandoc-bin        # al directorio actual
slacker download @alienbob              # repositorio entero (pide confirmación si >10)
```

slacker se niega a escribir a través de un symlink preexistente, así que
descargar en un directorio compartido como `/tmp` es seguro.

## 10. Congelar paquetes (blacklist)

Congela un paquete para que update, upgrade-all, reinstall y clean-system lo dejen
en paz (se añade a `/etc/slacker/blacklist`):

```
slacker frozen pandoc-bin               # congela uno
slacker frozen firefox chromium vlc     # congela varios
slacker frozen "@alienbob vlc-*"        # limita a un repo + un patrón (comillas obligatorias)
```

Pon entre comillas cualquier regla con un espacio (una regla `@repo`) o un carácter
glob de shell (`*`, `?`, `[`, `]`, ...). `"@alienbob vlc-*"` limita la regla al repo
`alienbob`, y `vlc-*` se compara como **regex sin anclar** contra el id completo —
en una regex `-*` significa "cero o más guiones", no "cualquier cosa", así que
congela cualquier paquete `alienbob` instalado cuyo id contenga `vlc` (p. ej.
`vlc`, `vlc-plugin-qt`), igual que haría un simple `vlc`. Para congelar solo el
paquete `vlc`, ánclala: `"@alienbob ^vlc-[0-9]"`.

Usa el nombre exacto del paquete (no el version-tag completo). Para descongelar,
quita la línea de `/etc/slacker/blacklist`.

La blacklist congela paquetes **individuales**. Para actuar sobre un **repositorio**
entero hay un mecanismo aparte — la *cuarentena*: `distrust-repo` congela un repo,
`vet-repo` lo revisa de nuevo, `trust-repo` lo libera (§8).

La blacklist es la manera por paquete de mantener algo fuera de `clean-system`.
Para proteger todo un grupo a la vez, prefiere `IGNORE_TAGS` (por build tag, p.
ej. `_SBo cf alien`) o marcar un repositorio `immutable` (§8, §12) en lugar de
congelar cada paquete a mano.

## 11. Plantillas (templates)

Una plantilla es una instantánea de los nombres de paquetes instalados que puedes
reproducir en otra máquina o tras una reinstalación.

```
slacker generate-template mybox         # instantánea de los paquetes actuales -> mybox.template
slacker install-template mybox          # instala todo lo que lista la plantilla
slacker remove-template mybox           # DESINSTALA cada paquete que lista la plantilla
slacker delete-template mybox           # elimina solo el archivo de plantilla (conserva los paquetes)
```

Nota la distinción: `remove-template` elimina los *paquetes*; `delete-template`
elimina solo el *archivo*.

## 12. Limpieza

```
slacker clean-system            # lista paquetes que ya no están en la baseline oficial, elige qué quitar
slacker --dry-run clean-system  # vista previa primero — hazlo siempre
slacker clean-cache             # elimina los *.txz descargados de la caché
slacker clean-cache alienbob    # solo los archivos en caché de ese repositorio
slacker --dry-run clean-cache   # muestra qué se liberaría
slacker new-config              # gestiona los archivos de configuración *.new sobrantes
```

`clean-cache` nunca toca los metadatos de los repositorios ni las claves GPG (que
viven bajo `CACHE_DIR/repos`), así que siempre es seguro ejecutarlo.

`clean-system` es al estilo slackpkg: elimina los paquetes que **ya no forman
parte de la baseline oficial** — el `PACKAGES.TXT` del repositorio oficial más
cualquier repositorio marcado `immutable`. Así, un paquete que la propia
distribución quitó se elimina aunque un repositorio de terceros aún sirva el
nombre. Un paquete se conserva (nunca se lista) cuando se cumple una de tres
cosas:

- coincide con una regla de **blacklist** (`slacker frozen NAME`);
- su **build tag** está en `IGNORE_TAGS` (`slacker.conf`), p. ej. `_SBo cf alien`;
- está atribuido a un repositorio **`immutable`** (el repositorio dueño de su
  build tag, o para un paquete sin tag cualquier repositorio immutable que provea
  su nombre).

Por eso, antes de tu primer `clean-system`, define `IGNORE_TAGS` para tus tags
SBo/locales/de fuente y/o marca los repositorios `extra/`/`testing/`/`patches/`
como `immutable` — de lo contrario esos paquetes aparecerán como ajenos. Como
salvaguarda, `clean-system` se niega a ejecutarse si un repositorio de la baseline
no tiene metadatos cargados (ejecuta `update` primero), y `--dry-run` muestra
exactamente qué eliminaría sin tocar nada.

## 13. Flags globales y códigos de salida

Los comandos de solo lectura (`search`, `info`, `file-search`, `check-updates`,
`show-changelog`) se ejecutan como cualquier usuario. Todo lo que cambia el
sistema, la caché o la configuración debe ejecutarse como root (o vía sudo por un
miembro de wheel); un intento no-root se detiene de inmediato con un mensaje
claro.

Esos comandos también toman un bloqueo exclusivo (`/run/slacker.lock`) para que
dos no puedan ejecutarse a la vez; una segunda invocación sale de inmediato
informando el PID en ejecución. El bloqueo se libera automáticamente si slacker
termina o es matado, así que un fallo nunca te deja bloqueado fuera. Las consultas
no toman bloqueo.

Flags (funcionan con cualquier comando):

```
--config-dir <DIR>    usar un directorio de configuración distinto (por defecto /etc/slacker)
-y, --yes             asume "sí" a todos los prompts
--dry-run             muestra qué pasaría, sin cambiar nada
--no-deps             no leer archivos .dep en esta ejecución
```

Códigos de salida:

```
0     éxito
1     error
20    nada encontrado / nada que hacer
50    hay una autoactualización de slacker disponible
100   actualizaciones pendientes (de check-updates)
```

Ejemplo de comprobación en un script:

```
slacker check-updates ; [ $? -eq 100 ] && slacker -y upgrade-all
```

## 14. Flujos de trabajo comunes

Actualización rutinaria del sistema:

```
slacker update
slacker upgrade-all
slacker --dry-run clean-system   # revisa primero (quita todo lo que esté fuera de la baseline)
slacker clean-system             # luego ejecútalo, una vez fijados IGNORE_TAGS/immutable (ver §12)
```

Primera sincronización tras editar repos, con importación de claves:

```
slacker update gpg
slacker update
slacker check-updates ; echo "exit=$?"
```

Vista previa de todo antes de comprometerse:

```
slacker --dry-run upgrade-all
slacker --dry-run install @gnome
slacker --dry-run clean-cache
```

Mover el conjunto de paquetes de una máquina a otra:

```
# en la máquina de origen
slacker generate-template snapshot
# copia /etc/slacker/templates/snapshot.template al destino, luego:
slacker update
slacker install-template snapshot
```

Liberar disco sin arriesgar metadatos ni claves:

```
slacker clean-cache
```

## 15. Verificación de paquetes

slacker verifica los paquetes antes de instalarlos. La política se fija
globalmente con `VERIFY` en `slacker.conf` y se puede anular por repositorio con
un flag `verify=` en la línea del repo.

Por defecto (`VERIFY=all`):

```
# slacker.conf
VERIFY=all
```

Con `all`, para cada paquete: la firma GPG se comprueba cuando el repositorio
proporciona una (una firma mala siempre falla; una ausente se omite), y al menos
una suma de integridad (md5 o sha) debe estar presente y coincidir. Si no hay ni
md5 ni sha, la instalación se detiene — el archivo de sumas del repositorio falta
o está roto.

Slackware incluye un `.txz.asc` por paquete junto a cada paquete, así que con
`all` slacker verifica con GPG el paquete en sí e imprime, p. ej.,
`verified: gpg (signer) + md5`. Para esto debes haber fijado la clave del
repositorio con `slacker update gpg`; hasta entonces obtienes
`integrity only: md5` (el paquete sigue verificado con md5 contra el
`CHECKSUMS.md5` firmado con GPG, solo que no autenticado por paquete). Para un
repositorio `subtree` la clave se obtiene de la raíz, donde Slackware guarda la
única clave que firma todo el árbol — así `extra/`/`testing/`/`patches/` fijan el
mismo fingerprint que el repositorio oficial.

**Fijación de clave (trust on first use):** la primera importación fija el
fingerprint del repositorio; si alguna vez cambia, slacker rechaza el repositorio
como posible ataque de sustitución de clave en lugar de confiar en la nueva en
silencio. Ver §8 para los comandos de cuarentena/confianza.

Exigir métodos concretos (se detiene si falta uno, diciéndote cómo relajarlo):

```
VERIFY=gpg,md5,sha
VERIFY=gpg,md5
VERIFY=md5
```

Desactivar por completo (no recomendado):

```
VERIFY=none
```

Anulación por repositorio — útil cuando un repositorio tiene una suma o firma rota
o ausente, para relajar solo ese en lugar de debilitarlo todo:

```
# repos
100  slackware  mirror                       official
80   conraid    https://slackers.it/...      verify=gpg,md5
60   alienbob   https://slackware.nl/...      verify=md5
```

Las mismas reglas se aplican a cada repositorio, incluido el oficial — no hay
exención. El flag `official` solo afecta al alcance de `install-new` y al
seguimiento del ChangeLog, no a la verificación.

Si una descarga falla la verificación verás un mensaje claro, por ejemplo:

```
md5 mismatch for foo-1.0-x86_64-1cf.txz: expected ..., got ...
no usable checksum (md5 or sha) for foo-...: the repo's checksum file may be
  missing or broken. ... relax verification for it with a `verify=` flag ...
```

---

<a id="zh"></a>
# 中文

`slacker` 的实用、以示例为主的指南。它是 Slackware 的二进制软件包管理器，将
`slackpkg` 与 `slackpkg+` 合二为一。

下面每个示例都是真实命令。会修改系统的操作（install、upgrade、remove 等）需要
root；查询类（search、info、file-search 等）则不需要。

## 目录

1. 首次配置
2. 保持元数据最新
3. 搜索、查看与列出仓库
4. 安装
5. 升级
6. 重新安装与移除
7. 整个仓库与 build tag（`@` 选择器）
8. 仓库管理（添加、删除、tag、信任）
9. 只下载不安装
10. 冻结软件包（blacklist）
11. 模板（templates）
12. 清理
13. 全局 flag 与退出码
14. 常见工作流
15. 软件包校验

## 1. 首次配置

配置位于 `/etc/slacker/`（可用 `--config-dir` 覆盖）。复制随附的模板并编辑：

```


```

在 `/etc/slacker/mirrors` 中只选择一个镜像（默认没有启用任何一个；启用两行或更多即
为错误）：

```
# /etc/slacker/mirrors  - uncomment ONE line
https://slackware.uk/slackware/slackware64-current/
```

在 `/etc/slacker/repos` 中声明你的仓库。二进制仓库使用一个 URL（官方仓库用关键字
`mirror`），且优先级必须**各不相同**；优先级最高者胜出：

```
# priority  name        url|mirror                                            [flags]
100         slackware   mirror                                                official
90          extras      https://mirror.nl.leaseweb.net/slackware/slackware64-current/extra   subtree
80          conraid     https://slackers.it/repository/slackware64-current
60          alienbob    https://slackware.nl/people/alien/sbrepos/current/x86_64
```

flag（顺序任意）放在 URL 之后：`official`（被跟踪的仓库）、`subtree`（Slackware
发行版子树——见下文）、`immutable`（其软件包永不被 `clean-system` 删除的仓库）以及
`verify=...`（按仓库覆盖校验策略）。

四个 Slackware 发行版子树——**`extra`、`patches`、`testing`、`pasture`**——
**必须始终带上 `subtree` flag**（在 URL 之后的任意位置）。它们的 `PACKAGES.TXT`
中软件包位置是相对于发行版根目录的，因此不加 `subtree` 其软件包会下载失败（路径段
重复）；加上之后，软件包与 `GPG-KEY` 从父级（根）URL 获取，而元数据仍来自仓库自身
的 URL。对这四个仓库而言这并非可选项。

可选地，添加 tag 优先级行，使来自源码/本地的软件包永不被 `upgrade-all` 迁移或降级：

```
100         SBo         _SBo
100         local       _rtz
```

导入一次各仓库的 GPG 密钥，然后刷新并检查配置：

```
slacker update gpg
slacker update
slacker status                  # 确认配置健康，并标出需要修正之处
```

`update gpg` 会固定（pin）每个仓库的密钥（trust on first use）。对 `subtree` 仓库，
它从根目录固定同一把 Slackware 密钥——正是这一点让 `extra/`/`testing/`/`patches/`
软件包在安装时能通过逐包 GPG 校验。

## 2. 保持元数据最新

```
slacker update                  # 刷新 PACKAGES.TXT/CHECKSUMS，校验 GPG
slacker update gpg              # （重新）导入 GPG 密钥，然后刷新
slacker check-updates           # 检查每个仓库；若有待更新则退出码 100
slacker show-changelog          # 查看缓存的 ChangeLog（在 TTY 上分页）
slacker show-changelog conraid  # 指定仓库的 ChangeLog（按需下载）
```

## 3. 搜索、查看与列出仓库

```
slacker search firefox          # 按精确名称查找软件包（不区分大小写）
slacker info bash               # 各仓库的候选 + 已安装版本
slacker file-search bin/bash    # 哪个软件包提供此文件（使用 MANIFEST）
slacker list-repos              # 仓库：优先级、已安装数量、verify、flag
slacker status                  # 对整个配置做健康检查；告诉你下一步该修什么
```

`info` 显示哪个仓库按优先级胜出。例如，若 `ffmpeg` 存在于多个仓库，候选为优先级最高
者；用 `repo:name` 固定为另一个（见下文）。

## 4. 安装

```
slacker install vlc                     # 一个软件包（+ 其 .dep 链）
slacker install vlc mpv obs-studio      # 一次多个
slacker install conraid:ffmpeg          # 强制使用 conraid 的构建（pin）
slacker --dry-run install vlc           # 仅预览，不做改动
slacker --no-deps install vlc           # 跳过依赖解析
slacker -y install vlc                  # 对所有提示一律“是”
```

若某个模式匹配到多个软件包，slacker 会打印一个带编号的列表：

```
slacker install python
# 'install' matched 12 packages:
#   1) [slackware] python-build-...
#   2) [slackware] python-cffi-...
#   ...
# Enter numbers to install (e.g. 1 3 5 or 2-4), Enter for all, 'n' to cancel:
```

已安装的软件包会被 `install` 拒绝（请用 `upgrade` 或 `reinstall`）。

**依赖是第三方功能，而非官方功能。** Slackware 自家的仓库——`slackware` 以及
`extra`/`patches`/`testing`/`pasture` 子树——既不提供也不期望依赖信息：官方前提是
**完整安装所有** Slackware 软件包集，因此每个依赖都被认为已经存在，slacker 不会为它们
做任何依赖解析（也不需要）。像 `alienbob` 和 `conraid` 这样的独立仓库确实为每个软件包
提供 `.dep` 文件；slacker **只对提供这些文件的仓库**读取它们，从同一仓库拉取缺失的
依赖。因此从官方树执行 `install`/`upgrade` 不解析任何依赖，而从带 `.dep` 文件的第三方
仓库执行则会解析。可随处用 `--no-deps`（按次运行）或 `RESOLVE_DEPS=no`（在
`slacker.conf` 中）关闭。

## 5. 升级

```
slacker upgrade vlc             # 升级指定软件包
slacker upgrade-all             # 升级所有有更新版本的软件包
slacker --dry-run upgrade-all   # 先预览整个升级计划
slacker -y upgrade-all          # 无提示
```

`upgrade-all` 尊重优先级与 build tag：软件包只会被来自优先级更高或相等仓库的候选替换，
因此 SBo/本地/源码软件包永远不会被悄悄迁移到另一个仓库或降级。它所需的新依赖会在确认
之前显示为 `new-dep: [repo] pkg (for parent)`。

安装自上次更新以来新加入发行版的软件包：

```
slacker install-new             # 仅官方仓库（默认）
slacker install-new conraid     # 仅 conraid 新加入的软件包
slacker install-new slackware extras
```

## 6. 重新安装与移除

```
slacker reinstall bash          # 重新安装当前版本
slacker reinstall y             # 重新安装整个系列（此处：games）
slacker reinstall ap            # 'ap' 系列
slacker remove libfoo           # 移除已安装的软件包
slacker --dry-run remove libfoo # 预览
```

系列名（`a`、`ap`、`d`、`k`、`kde`、`l`、`n`、`t`、`x`、`xap`、`xfce`、`y` 等）精确
匹配该系列，而不是名字里碰巧含这些字母的每个软件包。多重匹配仍会显示带编号的选择列表。

## 7. 整个仓库与 build tag（`@` 选择器）

`@` 前缀是显式的集合选择器。它是必需的——单独的一个词永远不会被当作仓库。

```
slacker install @gnome          # 安装 gnome 仓库的每个软件包
slacker remove  @gnome          # 移除来自该仓库的已安装软件包
slacker remove  @_SBo           # 移除所有已安装的 SlackBuilds.org 软件包
slacker download @alienbob      # 下载 alienbob 仓库的每个软件包
```

`@repo` 意为“该仓库中的每个软件包”；`@_tag` 意为“带该 build tag 的每个软件包”。拼写
错误会给出有用的错误提示：

```
slacker install @gnme
# error: unknown repo or tag '@gnme'; did you mean '@gnome'?
#   available repos: conraid, gnome, slackware
#   available tags:  _gnome, cf
```

典型用法：把一个桌面仓库（如 gnome）放在独立且较高的优先级（如 101），并作为集合安装。
其带 `_gnome` tag 的软件包随即被锁定——任何更低的仓库都无法替换或“升级”它们，即使有更
新版本：

```
# /etc/slacker/repos
101  gnome  https://your-gnome-repo/...
```
```
slacker update
slacker install @gnome
slacker upgrade-all             # 保持 gnome 软件包不动
```

## 8. 仓库管理（添加、删除、tag、信任）

你可以手动编辑 `/etc/slacker/repos`，或让 slacker 替你完成（带校验和确认提示）：

```
slacker add-repo 70 extras https://.../slackware64-current/extra subtree
slacker add-repo 80 conraid https://slackers.it/repository/slackware64-current
slacker del-repo conraid
slacker add-tag 100 SBo _SBo            # 一行 build-tag 优先级
slacker del-tag _SBo
```

`add-repo` 的 flag（顺序任意）：`official`、`immutable`、`subtree`、`verify=...`。
Slackware **子树**（`extra/`、`patches/`、`testing/`、`pasture/`）必须带 `subtree`，
否则其软件包无法下载；`immutable` 让某仓库的软件包不被 `clean-system` 删除（见 §12）。

随时查看与做健康检查：

```
slacker list-repos              # 优先级、已安装数量、verify 策略、flag
slacker status                  # 对整个配置做健康检查 + 该修什么
```

`list-repos` 显示一个表格，并标记 `(official)`、`(immutable)`、`(subtree)` 及任何
隔离状态。`status` 把发现分组（Setup / Installed / Online），用 ✓/!/✗ 标记，并以
通俗的结论和后续步骤收尾。

### 仓库安全：隔离（quarantine）与信任

slacker 会审查（vets）仓库，并把无法访问或提供畸形/恶意元数据的仓库放入**隔离**；被隔离
的仓库在你处理之前不提供任何软件包。新的或尚未受信的仓库会在每次 `update` 时被轻量审查；
`add-repo` 和 `vet-repo` 则做彻底审查。

```
slacker vet-repo conraid        # 按需重新检查（失败则隔离，通过则解除）
slacker trust-repo conraid      # 解除你判断为误报的隔离（覆盖判定）
slacker distrust-repo conraid   # 自行冻结某个仓库
```

GPG 密钥在首次导入时被固定（trust on first use）：若某仓库的密钥发生变化，slacker 会将
其拒绝为可能的密钥替换攻击，而不是默默信任新密钥。`list-repos` 和 `status` 会显示该状态。

## 9. 只下载不安装

文件默认保存到 `CACHE_DIR/packages/<repo>/`，正是 `install` 查找的位置，因此之后的
安装会复用它们。

```
slacker download pandoc-bin             # 到缓存
slacker download -o /tmp pandoc-bin     # 改为到 /tmp
slacker download -o . pandoc-bin        # 到当前目录
slacker download @alienbob              # 整个仓库（若 >10 会请求确认）
```

slacker 拒绝通过已存在的符号链接写入，因此下载到像 `/tmp` 这样的共享目录是安全的。

## 10. 冻结软件包（blacklist）

冻结某个软件包，使 update、upgrade-all、reinstall 和 clean-system 都不去动它（它会被
加入 `/etc/slacker/blacklist`）：

```
slacker frozen pandoc-bin               # 冻结一个
slacker frozen firefox chromium vlc     # 冻结多个
slacker frozen "@alienbob vlc-*"        # 限定到某仓库 + 一个模式（必须加引号）
```

凡是含空格（`@repo` 规则）或 shell glob 字符（`*`、`?`、`[`、`]` 等）的规则都要加引号。
`"@alienbob vlc-*"` 把规则限定到 `alienbob` 仓库，而 `vlc-*` 是按**未锚定的正则**匹配
完整 id 的——在正则里 `-*` 表示“零个或多个连字符”，而非“任意内容”，因此它会冻结任何已安装
的、id 中含 `vlc` 的 `alienbob` 软件包（如 `vlc`、`vlc-plugin-qt`），与单写 `vlc` 效果
相同。若只想冻结 `vlc` 这个软件包，请加锚点：`"@alienbob ^vlc-[0-9]"`。

使用精确的软件包名（不是完整的 version-tag）。要解冻，请从 `/etc/slacker/blacklist` 中
删除该行。

blacklist 冻结的是单个**软件包**。要对整个**仓库**采取行动，另有一套机制——*隔离*：
`distrust-repo` 冻结某仓库，`vet-repo` 重新检查它，`trust-repo` 解除隔离（§8）。

blacklist 是让某物不被 `clean-system` 处理的“逐包”方式。要一次性保护一整组，请优先使用
`IGNORE_TAGS`（按 build tag，如 `_SBo cf alien`）或把某仓库标记为 `immutable`
（§8、§12），而不是手动冻结每个软件包。

## 11. 模板（templates）

模板是已安装软件包名的快照，可在另一台机器上或重新安装后重放。

```
slacker generate-template mybox         # 当前软件包的快照 -> mybox.template
slacker install-template mybox          # 安装模板所列的一切
slacker remove-template mybox           # 卸载模板所列的每个软件包
slacker delete-template mybox           # 仅删除模板文件（保留软件包）
```

注意区别：`remove-template` 移除*软件包*；`delete-template` 只移除*文件*。

## 12. 清理

```
slacker clean-system            # 列出不再属于官方 baseline 的软件包，选择要移除哪些
slacker --dry-run clean-system  # 先预览——请务必这样做
slacker clean-cache             # 从缓存中删除已下载的 *.txz
slacker clean-cache alienbob    # 仅该仓库缓存的文件
slacker --dry-run clean-cache   # 显示将释放多少
slacker new-config              # 处理遗留的 *.new 配置文件
```

`clean-cache` 绝不触碰仓库元数据或 GPG 密钥（它们位于 `CACHE_DIR/repos` 下），因此
随时运行都安全。

`clean-system` 是 slackpkg 风格：它移除**不再属于官方 baseline** 的软件包——官方仓库
的 `PACKAGES.TXT` 加上任何标记为 `immutable` 的仓库。因此，即使某第三方仓库仍提供该
名称，被发行版本身移除的软件包也会被删除。当满足以下三者之一时，软件包会被保留（永不
列出）：

- 它匹配某条 **blacklist** 规则（`slacker frozen NAME`）；
- 它的 **build tag** 在 `IGNORE_TAGS`（`slacker.conf`）中，如 `_SBo cf alien`；
- 它归属于某个 **`immutable`** 仓库（拥有其 build tag 的仓库，或对无 tag 的软件包，
  任何提供其名称的 immutable 仓库）。

因此，在你第一次执行 `clean-system` 之前，请为你的 SBo/本地/源码 tag 设置
`IGNORE_TAGS`，和/或把 `extra/`/`testing/`/`patches/` 仓库标记为 `immutable`——否则
这些软件包会被列为外来。作为安全保护，若某个 baseline 仓库没有加载元数据，`clean-system`
会拒绝运行（请先 `update`），而 `--dry-run` 会准确显示它将移除什么，且不动任何东西。

## 13. 全局 flag 与退出码

只读命令（`search`、`info`、`file-search`、`check-updates`、`show-changelog`）可由
任何用户运行。任何会改变系统、缓存或配置的操作都必须以 root 运行（或由 wheel 成员通过
sudo）；非 root 的尝试会立即以清晰提示停止。

这些命令还会获取一个独占锁（`/run/slacker.lock`），使两个不能同时运行；第二次调用会
立即退出并报告正在运行的 PID。若 slacker 退出或被杀死，锁会自动释放，因此崩溃绝不会把你
锁在外面。查询不获取锁。

flag（适用于任何命令）：

```
--config-dir <DIR>    使用不同的配置目录（默认 /etc/slacker）
-y, --yes             对所有提示一律“是”
--dry-run             显示将会发生什么，不做任何改动
--no-deps             本次运行不读取 .dep 文件
```

退出码：

```
0     成功
1     错误
20    未找到 / 无事可做
50    有可用的 slacker 自升级
100   有待更新（来自 check-updates）
```

脚本中的检查示例：

```
slacker check-updates ; [ $? -eq 100 ] && slacker -y upgrade-all
```

## 14. 常见工作流

例行系统更新：

```
slacker update
slacker upgrade-all
slacker --dry-run clean-system   # 先审阅（它会移除任何不在 baseline 中的东西）
slacker clean-system             # 在设置好 IGNORE_TAGS/immutable 后再运行（见 §12）
```

编辑 repos 后的首次同步，并导入密钥：

```
slacker update gpg
slacker update
slacker check-updates ; echo "exit=$?"
```

在提交前预览一切：

```
slacker --dry-run upgrade-all
slacker --dry-run install @gnome
slacker --dry-run clean-cache
```

把一台机器的软件包集合迁移到另一台：

```
# 在源机器上
slacker generate-template snapshot
# 把 /etc/slacker/templates/snapshot.template 复制到目标，然后：
slacker update
slacker install-template snapshot
```

在不冒元数据或密钥风险的前提下释放磁盘：

```
slacker clean-cache
```

## 15. 软件包校验

slacker 在安装前会校验软件包。策略通过 `slacker.conf` 中的 `VERIFY` 全局设置，并可在
repos 行上用 `verify=` flag 按仓库覆盖。

默认（`VERIFY=all`）：

```
# slacker.conf
VERIFY=all
```

使用 `all` 时，对每个软件包：当仓库提供 GPG 签名时进行校验（坏签名总是失败；缺失则跳过），
且至少要有一个完整性校验和（md5 或 sha）存在并匹配。如果 md5 与 sha 都不可用，安装会
停止——该仓库的校验和文件缺失或损坏。

Slackware 在每个软件包旁都放有一个逐包的 `.txz.asc`，因此在 `all` 下 slacker 会对软件
包本身做 GPG 校验并打印如 `verified: gpg (signer) + md5`。为此你必须已用
`slacker update gpg` 固定该仓库的密钥；在此之前你会得到 `integrity only: md5`（软件包
仍会以 md5 对照经 GPG 签名的 `CHECKSUMS.md5` 校验，只是没有逐包认证）。对 `subtree`
仓库，密钥从根目录获取——Slackware 在那里保存着为整棵树签名的那一把密钥——因此
`extra/`/`testing/`/`patches/` 固定的 fingerprint 与官方仓库相同。

**密钥固定（trust on first use）：** 首次导入会固定该仓库的 fingerprint；若它日后发生
变化，slacker 会将该仓库拒绝为可能的密钥替换攻击，而不是默默信任新密钥。隔离/信任命令见
§8。

要求特定方法（缺少其一即停止，并告诉你如何放宽）：

```
VERIFY=gpg,md5,sha
VERIFY=gpg,md5
VERIFY=md5
```

完全禁用（不建议）：

```
VERIFY=none
```

按仓库覆盖——当某仓库的校验和或签名损坏或缺失时很有用，可只放宽该仓库，而不削弱全部：

```
# repos
100  slackware  mirror                       official
80   conraid    https://slackers.it/...      verify=gpg,md5
60   alienbob   https://slackware.nl/...      verify=md5
```

同样的规则适用于每个仓库，包括官方仓库——没有豁免。`official` flag 只影响 `install-new`
的范围与 ChangeLog 跟踪，不影响校验。

若某次下载未通过校验，你会看到清晰的消息，例如：

```
md5 mismatch for foo-1.0-x86_64-1cf.txz: expected ..., got ...
no usable checksum (md5 or sha) for foo-...: the repo's checksum file may be
  missing or broken. ... relax verification for it with a `verify=` flag ...
```

---

<a id="fr"></a>
# Français

Un guide pratique et fondé sur des exemples de `slacker`, un gestionnaire de
paquets binaires pour Slackware qui réunit `slackpkg` et `slackpkg+` en un seul
outil.

Chaque exemple ci-dessous est une commande réelle. Les actions qui modifient le
système (install, upgrade, remove, ...) nécessitent root ; les interrogations
(search, info, file-search, ...) non.

## Sommaire

1. Configuration initiale
2. Garder les métadonnées à jour
3. Recherche, inspection et liste des dépôts
4. Installation
5. Mise à niveau
6. Réinstallation et suppression
7. Dépôts entiers et build tags (sélecteurs `@`)
8. Gestion des dépôts (ajouter, supprimer, tags, confiance)
9. Télécharger sans installer
10. Geler des paquets (blacklist)
11. Modèles (templates)
12. Nettoyage
13. Flags globaux et codes de sortie
14. Flux de travail courants
15. Vérification des paquets

## 1. Configuration initiale

La configuration se trouve dans `/etc/slacker/` (remplaçable par `--config-dir`).
Copie les modèles fournis et édite-les :

```


```

Choisis exactement un miroir dans `/etc/slacker/mirrors` (aucun n'est actif par
défaut ; deux lignes actives ou plus est une erreur) :

```
# /etc/slacker/mirrors  - uncomment ONE line
https://slackware.uk/slackware/slackware64-current/
```

Déclare tes dépôts dans `/etc/slacker/repos`. Les dépôts binaires prennent une
URL (ou le mot-clé `mirror` pour l'officiel) et doivent avoir des priorités
**distinctes** ; la plus haute l'emporte :

```
# priority  name        url|mirror                                            [flags]
100         slackware   mirror                                                official
90          extras      https://mirror.nl.leaseweb.net/slackware/slackware64-current/extra   subtree
80          conraid     https://slackers.it/repository/slackware64-current
60          alienbob    https://slackware.nl/people/alien/sbrepos/current/x86_64
```

Les flags (dans n'importe quel ordre) viennent après l'URL : `official` (le dépôt
suivi), `subtree` (un sous-arbre de la distribution Slackware — voir plus bas),
`immutable` (un dépôt dont `clean-system` ne supprime jamais les paquets) et
`verify=...` (surcharge de la vérification par dépôt).

Les quatre sous-arbres de la distribution Slackware — **`extra`, `patches`,
`testing`, `pasture`** — **doivent toujours porter le flag `subtree`** (à
n'importe quelle position après l'URL). Leur `PACKAGES.TXT` indique des
emplacements de paquets relatifs à la racine de la distribution, donc sans
`subtree` leurs paquets échouent au téléchargement (un segment de chemin
dédoublé) ; avec lui, les paquets et `GPG-KEY` sont récupérés depuis l'URL parente
(racine) tandis que les métadonnées proviennent toujours de l'URL du dépôt
lui-même. Ce n'est pas optionnel pour ces quatre dépôts.

Optionnellement, ajoute des lignes de priorité de tag pour que les paquets
source/locaux ne soient jamais migrés ni rétrogradés par `upgrade-all` :

```
100         SBo         _SBo
100         local       _rtz
```

Importe une fois les clés GPG des dépôts, puis rafraîchis et vérifie la
configuration :

```
slacker update gpg
slacker update
slacker status                  # confirme que la configuration est saine et signale ce qu'il faut corriger
```

`update gpg` épingle (pin) la clé de chaque dépôt (trust on first use). Pour un
dépôt `subtree`, il épingle la même clé Slackware depuis la racine — c'est
précisément ce qui permet à la vérification GPG par paquet de réussir pour les
paquets `extra/`/`testing/`/`patches/` à l'installation.

## 2. Garder les métadonnées à jour

```
slacker update                  # rafraîchit PACKAGES.TXT/CHECKSUMS, vérifie GPG
slacker update gpg              # (ré)importe les clés GPG, puis rafraîchit
slacker check-updates           # vérifie chaque dépôt ; code 100 si une mise à jour est en attente
slacker show-changelog          # affiche le ChangeLog en cache (paginé sur un TTY)
slacker show-changelog conraid  # ChangeLog d'un dépôt nommé (téléchargé à la demande)
```

## 3. Recherche, inspection et liste des dépôts

```
slacker search firefox          # trouve un paquet par son nom exact (insensible à la casse)
slacker info bash               # candidats par dépôt + version installée
slacker file-search bin/bash    # quel paquet fournit ce fichier (utilise MANIFEST)
slacker list-repos              # dépôts : priorité, nombre d'installés, verify, flags
slacker status                  # bilan de santé de toute la configuration ; dit quoi corriger
```

`info` montre quel dépôt l'emporte par priorité. Par exemple, si `ffmpeg` existe
dans plusieurs dépôts, le candidat est celui de plus haute priorité ; épingle-en
un autre avec `repo:name` (voir plus bas).

## 4. Installation

```
slacker install vlc                     # un paquet (+ sa chaîne de .dep)
slacker install vlc mpv obs-studio      # plusieurs à la fois
slacker install conraid:ffmpeg          # force la build de conraid (pin)
slacker --dry-run install vlc           # aperçu seulement, ne change rien
slacker --no-deps install vlc           # saute la résolution des dépendances
slacker -y install vlc                  # répond « oui » à toutes les invites
```

Si un motif correspond à plus d'un paquet, slacker imprime une liste numérotée :

```
slacker install python
# 'install' matched 12 packages:
#   1) [slackware] python-build-...
#   2) [slackware] python-cffi-...
#   ...
# Enter numbers to install (e.g. 1 3 5 or 2-4), Enter for all, 'n' to cancel:
```

Les paquets déjà installés sont refusés par `install` (utilise `upgrade` ou
`reinstall`).

**Les dépendances sont une fonctionnalité tierce, pas officielle.** Les dépôts
propres à Slackware — `slackware` et les sous-arbres
`extra`/`patches`/`testing`/`pasture` — ne fournissent ni n'attendent
d'information de dépendances : une installation complète de **tous** les jeux de
paquets Slackware est le prérequis officiel, donc chaque dépendance est supposée
déjà présente, et slacker n'effectue aucune résolution de dépendances pour eux
(inutile). Des dépôts indépendants comme `alienbob` et `conraid` fournissent bien
des fichiers `.dep` par paquet ; slacker les lit **uniquement pour les dépôts qui
les fournissent**, tirant les dépendances manquantes depuis ce même dépôt. Ainsi
un `install`/`upgrade` depuis l'arbre officiel ne résout rien, alors qu'un depuis
un dépôt tiers avec des fichiers `.dep` le fait. Désactive-le partout avec
`--no-deps` (par exécution) ou `RESOLVE_DEPS=no` (dans `slacker.conf`).

## 5. Mise à niveau

```
slacker upgrade vlc             # met à niveau des paquets précis
slacker upgrade-all             # met à niveau tout ce qui a une révision plus récente
slacker --dry-run upgrade-all   # aperçu de tout le plan de mise à niveau d'abord
slacker -y upgrade-all          # sans invites
```

`upgrade-all` respecte la priorité et les build tags : un paquet n'est remplacé
que par un candidat d'un dépôt de priorité supérieure ou égale, donc les paquets
SBo/locaux/source ne sont jamais migrés silencieusement vers un autre dépôt ni
rétrogradés. Les nouvelles dépendances nécessaires sont montrées avant la
confirmation sous la forme `new-dep: [repo] pkg (for parent)`.

Installer les paquets ajoutés à la distribution depuis ta dernière mise à jour :

```
slacker install-new             # dépôts officiels seulement (par défaut)
slacker install-new conraid     # seulement les paquets récemment ajoutés de conraid
slacker install-new slackware extras
```

## 6. Réinstallation et suppression

```
slacker reinstall bash          # réinstalle la version actuelle
slacker reinstall y             # réinstalle une série entière (ici : games)
slacker reinstall ap            # la série 'ap'
slacker remove libfoo           # supprime des paquets installés
slacker --dry-run remove libfoo # aperçu
```

Les noms de série (`a`, `ap`, `d`, `k`, `kde`, `l`, `n`, `t`, `x`, `xap`,
`xfce`, `y`, ...) correspondent exactement à cette série, pas à chaque paquet
dont le nom contient ces lettres. Une correspondance multiple affiche tout de
même la liste numérotée de sélection.

## 7. Dépôts entiers et build tags (sélecteurs `@`)

Le préfixe `@` est un sélecteur d'ensemble explicite. Il est obligatoire — un mot
simple n'est jamais traité comme un dépôt.

```
slacker install @gnome          # installe chaque paquet du dépôt gnome
slacker remove  @gnome          # supprime les paquets installés provenant de ce dépôt
slacker remove  @_SBo           # supprime tous les paquets SlackBuilds.org installés
slacker download @alienbob      # télécharge chaque paquet du dépôt alienbob
```

`@repo` signifie « chaque paquet de ce dépôt » ; `@_tag` signifie « chaque paquet
avec ce build tag ». Une faute de frappe donne une erreur utile :

```
slacker install @gnme
# error: unknown repo or tag '@gnme'; did you mean '@gnome'?
#   available repos: conraid, gnome, slackware
#   available tags:  _gnome, cf
```

Usage typique : place un dépôt de bureau (p. ex. gnome) à une priorité haute et
distincte comme 101 et installe-le comme un ensemble. Ses paquets tagués `_gnome`
sont alors verrouillés — aucun dépôt inférieur ne peut les remplacer ni les
« mettre à niveau », même avec une version plus récente :

```
# /etc/slacker/repos
101  gnome  https://your-gnome-repo/...
```
```
slacker update
slacker install @gnome
slacker upgrade-all             # laisse les paquets gnome intacts
```

## 8. Gestion des dépôts (ajouter, supprimer, tags, confiance)

Tu peux éditer `/etc/slacker/repos` à la main, ou laisser slacker le faire pour
toi (avec validation et invite de confirmation) :

```
slacker add-repo 70 extras https://.../slackware64-current/extra subtree
slacker add-repo 80 conraid https://slackers.it/repository/slackware64-current
slacker del-repo conraid
slacker add-tag 100 SBo _SBo            # une ligne de priorité de build-tag
slacker del-tag _SBo
```

Flags de `add-repo` (n'importe quel ordre) : `official`, `immutable`, `subtree`,
`verify=...`. Un **subtree** Slackware (`extra/`, `patches/`, `testing/`,
`pasture/`) doit recevoir `subtree` sinon ses paquets échouent au
téléchargement ; `immutable` garde les paquets d'un dépôt hors de `clean-system`
(voir §12).

Inspecte et fais un bilan de santé à tout moment :

```
slacker list-repos              # priorité, nombre d'installés, politique verify, flags
slacker status                  # bilan de santé de toute la configuration + quoi corriger
```

`list-repos` montre un tableau et marque `(official)`, `(immutable)`, `(subtree)`
et toute mise en quarantaine. `status` regroupe ses constats (Setup / Installed /
Online) avec des marqueurs ✓/!/✗ et se termine par un verdict en langage clair et
les étapes suivantes.

### Sécurité des dépôts : quarantaine et confiance

slacker contrôle (vets) les dépôts et met en **quarantaine** ceux qui sont
injoignables ou servent des métadonnées malformées/hostiles ; un dépôt en
quarantaine ne fournit aucun paquet jusqu'à ce que tu agisses. Les dépôts
nouveaux ou pas encore approuvés sont contrôlés légèrement à chaque `update` ;
`add-repo` et `vet-repo` contrôlent en profondeur.

```
slacker vet-repo conraid        # recontrôle à la demande (quarantaine si échec, levée si succès)
slacker trust-repo conraid      # lève une quarantaine que tu juges être un faux positif (override)
slacker distrust-repo conraid   # gèle toi-même un dépôt
```

Les clés GPG sont épinglées à la première importation (trust on first use) : si
la clé d'un dépôt change un jour, slacker le refuse comme une possible attaque de
substitution de clé plutôt que de faire silencieusement confiance à la nouvelle.
`list-repos` et `status` montrent l'état.

## 9. Télécharger sans installer

Les fichiers sont enregistrés dans `CACHE_DIR/packages/<repo>/` par défaut, le
même endroit où regarde `install`, afin qu'une installation ultérieure les
réutilise.

```
slacker download pandoc-bin             # dans le cache
slacker download -o /tmp pandoc-bin     # dans /tmp à la place
slacker download -o . pandoc-bin        # dans le répertoire courant
slacker download @alienbob              # dépôt entier (demande confirmation si >10)
```

slacker refuse d'écrire à travers un lien symbolique préexistant, donc télécharger
dans un répertoire partagé comme `/tmp` est sûr.

## 10. Geler des paquets (blacklist)

Gèle un paquet pour que update, upgrade-all, reinstall et clean-system le laissent
tranquille (il est ajouté à `/etc/slacker/blacklist`) :

```
slacker frozen pandoc-bin               # gèle-en un
slacker frozen firefox chromium vlc     # gèle-en plusieurs
slacker frozen "@alienbob vlc-*"        # restreint à un dépôt + un motif (guillemets obligatoires)
```

Mets entre guillemets toute règle contenant un espace (une règle `@repo`) ou un
caractère glob du shell (`*`, `?`, `[`, `]`, ...). `"@alienbob vlc-*"` restreint la
règle au dépôt `alienbob`, et `vlc-*` est comparé comme une **regex non ancrée** à
l'id complet — dans une regex `-*` veut dire « zéro ou plusieurs tirets », pas
« n'importe quoi », donc elle gèle tout paquet `alienbob` installé dont l'id
contient `vlc` (p. ex. `vlc`, `vlc-plugin-qt`), exactement comme le ferait un
simple `vlc`. Pour ne geler que le paquet `vlc`, ancre-la : `"@alienbob ^vlc-[0-9]"`.

Utilise le nom exact du paquet (pas le version-tag complet). Pour dégeler, retire
la ligne de `/etc/slacker/blacklist`.

La blacklist gèle des **paquets** individuels. Pour agir sur un **dépôt** entier
il existe un mécanisme distinct — la *quarantaine* : `distrust-repo` gèle un dépôt,
`vet-repo` le recontrôle, `trust-repo` le libère (§8).

La blacklist est la façon « par paquet » de garder quelque chose hors de
`clean-system`. Pour protéger tout un groupe d'un coup, préfère `IGNORE_TAGS` (par
build tag, p. ex. `_SBo cf alien`) ou marquer un dépôt `immutable` (§8, §12)
plutôt que de geler chaque paquet à la main.

## 11. Modèles (templates)

Un modèle est un instantané des noms de paquets installés que tu peux rejouer sur
une autre machine ou après une réinstallation.

```
slacker generate-template mybox         # instantané des paquets actuels -> mybox.template
slacker install-template mybox          # installe tout ce que le modèle liste
slacker remove-template mybox           # DÉSINSTALLE chaque paquet listé par le modèle
slacker delete-template mybox           # supprime seulement le fichier modèle (garde les paquets)
```

Note la distinction : `remove-template` supprime les *paquets* ; `delete-template`
supprime seulement le *fichier*.

## 12. Nettoyage

```
slacker clean-system            # liste les paquets qui ne sont plus dans la baseline officielle, choisis ce qu'il faut retirer
slacker --dry-run clean-system  # aperçu d'abord — fais-le toujours
slacker clean-cache             # supprime les *.txz téléchargés du cache
slacker clean-cache alienbob    # seulement les fichiers en cache de ce dépôt
slacker --dry-run clean-cache   # montre ce qui serait libéré
slacker new-config              # gère les fichiers de configuration *.new restants
```

`clean-cache` ne touche jamais aux métadonnées des dépôts ni aux clés GPG (qui
vivent sous `CACHE_DIR/repos`), donc il est toujours sûr de l'exécuter.

`clean-system` est de style slackpkg : il retire les paquets qui **ne font plus
partie de la baseline officielle** — le `PACKAGES.TXT` du dépôt officiel plus tout
dépôt marqué `immutable`. Ainsi, un paquet que la distribution elle-même a retiré
est supprimé même si un dépôt tiers fournit encore le nom. Un paquet est conservé
(jamais listé) quand l'une de ces trois conditions est vraie :

- il correspond à une règle **blacklist** (`slacker frozen NAME`) ;
- son **build tag** est dans `IGNORE_TAGS` (`slacker.conf`), p. ex. `_SBo cf alien` ;
- il est attribué à un dépôt **`immutable`** (le dépôt qui possède son build tag,
  ou pour un paquet sans tag, tout dépôt immutable qui en fournit le nom).

Donc avant ton premier `clean-system`, définis `IGNORE_TAGS` pour tes tags
SBo/locaux/source et/ou marque les dépôts `extra/`/`testing/`/`patches/` comme
`immutable` — sinon ces paquets apparaîtront comme étrangers. Par sécurité,
`clean-system` refuse de s'exécuter si un dépôt de la baseline n'a pas de
métadonnées chargées (exécute `update` d'abord), et `--dry-run` montre exactement
ce qu'il retirerait sans rien toucher.

## 13. Flags globaux et codes de sortie

Les commandes en lecture seule (`search`, `info`, `file-search`, `check-updates`,
`show-changelog`) s'exécutent en tant que n'importe quel utilisateur. Tout ce qui
change le système, le cache ou la configuration doit être exécuté en root (ou via
sudo par un membre de wheel) ; une tentative non-root s'arrête aussitôt avec un
message clair.

Ces commandes prennent aussi un verrou exclusif (`/run/slacker.lock`) afin que
deux ne puissent pas s'exécuter en même temps ; une seconde invocation sort
immédiatement en signalant le PID en cours. Le verrou est libéré automatiquement
si slacker se termine ou est tué, donc un plantage ne te bloque jamais dehors. Les
interrogations ne prennent pas de verrou.

Flags (fonctionnent avec toute commande) :

```
--config-dir <DIR>    utiliser un autre répertoire de configuration (par défaut /etc/slacker)
-y, --yes             répond « oui » à toutes les invites
--dry-run             montre ce qui se passerait, sans rien changer
--no-deps             ne pas lire les fichiers .dep pour cette exécution
```

Codes de sortie :

```
0     succès
1     erreur
20    rien trouvé / rien à faire
50    une auto-mise-à-niveau de slacker est disponible
100   mises à jour en attente (de check-updates)
```

Exemple de vérification dans un script :

```
slacker check-updates ; [ $? -eq 100 ] && slacker -y upgrade-all
```

## 14. Flux de travail courants

Mise à jour de routine du système :

```
slacker update
slacker upgrade-all
slacker --dry-run clean-system   # examine d'abord (il retire tout ce qui est hors baseline)
slacker clean-system             # puis exécute-le, une fois IGNORE_TAGS/immutable définis (voir §12)
```

Première synchronisation après édition des repos, avec import des clés :

```
slacker update gpg
slacker update
slacker check-updates ; echo "exit=$?"
```

Aperçu de tout avant de t'engager :

```
slacker --dry-run upgrade-all
slacker --dry-run install @gnome
slacker --dry-run clean-cache
```

Déplacer l'ensemble de paquets d'une machine vers une autre :

```
# sur la machine source
slacker generate-template snapshot
# copie /etc/slacker/templates/snapshot.template sur la cible, puis :
slacker update
slacker install-template snapshot
```

Libérer de l'espace sans risquer métadonnées ni clés :

```
slacker clean-cache
```

## 15. Vérification des paquets

slacker vérifie les paquets avant de les installer. La politique se règle
globalement avec `VERIFY` dans `slacker.conf` et peut être surchargée par dépôt
avec un flag `verify=` sur la ligne du repo.

Par défaut (`VERIFY=all`) :

```
# slacker.conf
VERIFY=all
```

Avec `all`, pour chaque paquet : la signature GPG est vérifiée lorsque le dépôt en
fournit une (une mauvaise signature échoue toujours ; une absente est ignorée), et
au moins une somme d'intégrité (md5 ou sha) doit être présente et correspondre. Si
ni md5 ni sha ne sont disponibles, l'installation s'arrête — le fichier de sommes
du dépôt manque ou est cassé.

Slackware fournit un `.txz.asc` par paquet à côté de chaque paquet, donc sous
`all` slacker vérifie via GPG le paquet lui-même et affiche, p. ex.,
`verified: gpg (signer) + md5`. Pour cela tu dois avoir épinglé la clé du dépôt
avec `slacker update gpg` ; jusque-là tu obtiens `integrity only: md5` (le paquet
est tout de même vérifié en md5 par rapport au `CHECKSUMS.md5` signé GPG, juste
pas authentifié par paquet). Pour un dépôt `subtree`, la clé est récupérée depuis
la racine, où Slackware garde l'unique clé qui signe tout l'arbre — ainsi
`extra/`/`testing/`/`patches/` épinglent le même fingerprint que le dépôt officiel.

**Épinglage de clé (trust on first use) :** la première importation épingle le
fingerprint du dépôt ; s'il change un jour, slacker refuse le dépôt comme une
possible attaque de substitution de clé plutôt que de faire silencieusement
confiance à la nouvelle. Voir §8 pour les commandes de quarantaine/confiance.

Exiger des méthodes précises (s'arrête s'il en manque une, en t'indiquant comment
l'assouplir) :

```
VERIFY=gpg,md5,sha
VERIFY=gpg,md5
VERIFY=md5
```

Désactiver entièrement (déconseillé) :

```
VERIFY=none
```

Surcharge par dépôt — utile quand un dépôt a une somme ou une signature cassée ou
absente, pour n'assouplir que celui-là au lieu de tout affaiblir :

```
# repos
100  slackware  mirror                       official
80   conraid    https://slackers.it/...      verify=gpg,md5
60   alienbob   https://slackware.nl/...      verify=md5
```

Les mêmes règles s'appliquent à chaque dépôt, y compris l'officiel — sans
exception. Le flag `official` n'affecte que la portée de `install-new` et le suivi
du ChangeLog, pas la vérification.

Si un téléchargement échoue à la vérification, tu verras un message clair, par
exemple :

```
md5 mismatch for foo-1.0-x86_64-1cf.txz: expected ..., got ...
no usable checksum (md5 or sha) for foo-...: the repo's checksum file may be
  missing or broken. ... relax verification for it with a `verify=` flag ...
```
