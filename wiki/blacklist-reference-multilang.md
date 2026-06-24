<a id="top"></a>
# slacker blacklist reference

Scegli la tua lingua / Выберите язык / Elige tu idioma / 选择语言 / Choisissez votre langue :

- [Italiano](#it)
- [Русский](#ru)
- [Español](#es)
- [中文](#zh)
- [Français](#fr)

---

<a id="it"></a>
# Italiano

Come aggiungere pacchetti e pattern alla blacklist di slacker, sia con il comando
`frozen` sia modificando il file a mano. Questo è un catalogo completo delle
forme delle regole e dei comandi che le creano.

## Cosa fa la blacklist

Una regola della blacklist seleziona dei pacchetti. L'effetto dipende dal fatto
che il pacchetto selezionato sia installato o no:

- Installato e selezionato: il pacchetto viene **congelato** (frozen). slacker
  non lo installa, aggiorna, reinstalla o rimuove mai, e non compare mai in
  `clean-system`.
- Non installato e selezionato: il pacchetto viene **nascosto** da
  `install-new`, dagli aggiornamenti e da `check-updates`. Compare ancora in
  `search` e `info`, contrassegnato con `[blacklisted]`.

## Tenere un pacchetto fuori da clean-system

Congelare un pacchetto con la blacklist è **uno dei tre** modi per tenerlo fuori
da `clean-system`, che rimuove i pacchetti non più presenti nella base ufficiale
(il `PACKAGES.TXT` ufficiale più ogni repo marcato `immutable`):

- una regola di **blacklist** — per singolo pacchetto (questo file);
- il suo **build tag** in `IGNORE_TAGS` (in `slacker.conf`), es. `_SBo cf alien`;
- il suo repo marcato **`immutable`** in `repos`, che mantiene ogni pacchetto
  attribuito a quel repo.

Per un intero subtree di Slackware (`extra/`, `testing/`, `patches/`) è
preferibile marcare il repo come `immutable` invece di mettere in blacklist ogni
pacchetto a mano.

## Sintassi delle regole

Ogni voce è una regola:

```
[@repo] PATTERN
```

- `PATTERN` che termina con `/` è una regola di **serie** (es. `kde/`); seleziona
  una serie Slackware.
- Qualsiasi altro `PATTERN` è un'**espressione regolare** non ancorata,
  confrontata con l'id completo del pacchetto `nome-versione-arch-build` (stile
  slackpkg).
- Il prefisso facoltativo `@repo` limita la regola a un singolo repository. Per
  un pacchetto disponibile è il repo candidato; per un pacchetto installato è il
  repo da cui proviene.

Poiché la regex è confrontata con l'id intero ed è non ancorata:

- un semplice `vlc` seleziona `vlc` e anche `vlc-plugin-*` (sottostringa),
- `^vlc-[0-9]` seleziona solo il pacchetto `vlc`,
- `xf86-.*-202.*` seleziona le build con versione `202x`,
- usa `^` e `$` per una selezione esatta.

Il repository ufficiale è semplicemente un repo con un nome. Usa quel nome con
`@`. In una configurazione predefinita si chiama `slackware`, quindi le regole
solo-ufficiali sono `@slackware PATTERN`.

## Metodo 1: il comando `frozen`

```
slacker frozen REGOLA [REGOLA ...]
```

Ogni argomento è una regola. Il comando convalida ogni regola, mostra cosa
congelerà, chiede conferma, poi aggiunge le nuove regole a
`/etc/slacker/blacklist`. Richiede i permessi di root.

### Virgolette

Le virgolette servono alla shell, non a slacker. Usa le virgolette doppie quando
una regola:

- contiene uno spazio (ogni regola `@repo PATTERN`), oppure
- contiene un carattere speciale della shell: `*` `?` `[` `]` `(` `)` `|` `$`.

Nomi semplici e `serie/` non richiedono virgolette. Nel dubbio, usale: le
virgolette doppie non fanno mai male.

```
slacker frozen vlc                 # va bene senza virgolette
slacker frozen kde/                # va bene senza virgolette
slacker frozen "xlibre-*"          # OBBLIGATORIE: * è un glob della shell
slacker frozen "@alienbob vlc"     # OBBLIGATORIE: contiene uno spazio
slacker frozen "@alienbob vlc-*"   # OBBLIGATORIE: uno spazio (@repo) E un glob *
```

Cosa fa davvero `"@alienbob vlc-*"`: `@alienbob` limita la regola al repo
`alienbob`, e `vlc-*` è confrontato come **regex non ancorata** sull'id completo —
in una regex `-*` significa "zero o più trattini", non "qualsiasi cosa". Quindi
congela ogni pacchetto `alienbob` il cui id **contiene** `vlc` (es. `vlc`,
`vlc-plugin-qt`), proprio come un semplice `vlc`; il `-*` non aggiunge nulla. Per
congelare solo `vlc`, ancorala: `"@alienbob ^vlc-[0-9]"` (per "vlc seguito da
qualsiasi cosa" la regex è `vlc-.*`, non `vlc-*`).

### Congelare un pacchetto vs. mettere in quarantena un repo

La blacklist congela singoli **pacchetti**. Agire su un intero **repository** è un
meccanismo separato — la *quarantena*: `distrust-repo` mette in quarantena un repo
tu stesso, `vet-repo` lo ri-controlla su richiesta, `trust-repo` toglie una
quarantena che giudichi un falso positivo. slacker mette anche automaticamente in
quarantena un repo che fallisce la verifica (irraggiungibile o che serve metadati
malformati/ostili); un repo in quarantena non fornisce alcun pacchetto. Regola
pratica: blacklist per i pacchetti, quarantena per i repository.

### Una o più regole

```
slacker frozen vlc
slacker frozen vlc "xf86-.*-202.*" "kde/" "@alienbob discover"
```

### Forme

```
# singolo pacchetto (attenzione: seleziona anche i nomi che lo contengono)
slacker frozen mozilla-firefox

# solo quel pacchetto, in modo stretto
slacker frozen "^mozilla-firefox-[0-9]"

# regex su tutti i repo
slacker frozen "xf86-.*-202.*"
slacker frozen "xlibre-.*"

# un'intera serie (tutti i repo)
slacker frozen kde/

# limita a un repo: un pacchetto da un repo specifico
slacker frozen "@alienbob discover"

# limita a un repo: un'intera serie da un repo specifico
slacker frozen "@conraid kde/"

# solo repo ufficiale (nome predefinito: slackware)
slacker frozen "@slackware mozilla-firefox"
slacker frozen "@slackware kde/"
slacker frozen "@slackware ^mozilla-firefox-"
```

### Convalida e avvisi

- Un errore di sintassi (per esempio `@repo` senza pattern) è fatale: non viene
  scritto nulla e tutti i problemi vengono elencati insieme, così si correggono
  in una sola volta.
- Una regola che è valida ma sembra un errore viene segnalata e ti viene chiesto
  se dichiararla comunque:
  - un `@repo` che non corrisponde a nessun repository attivo (probabile errore
    di battitura),
  - una regex che contiene uno spazio; gli id dei pacchetti non contengono mai
    spazi, quindi di solito significa una `@` dimenticata (per esempio
    `conraid foo` invece di `@conraid foo`) o un errore di virgolette.
- Le regole già presenti nel file vengono saltate, e il conteggio nella conferma
  riflette solo le regole effettivamente aggiunte.

### Opzioni

- `--yes` (o `-y`): salta gli avvisi e la conferma.
- `--config-dir DIR`: usa una blacklist sotto `DIR` invece di `/etc/slacker`.

## Metodo 2: modificare il file a mano

La blacklist è un file di testo semplice in:

```
/etc/slacker/blacklist
```

Una regola per riga. Le righe vuote vengono ignorate. Un `#` inizia un commento
(intera riga o finale). Modificalo con qualsiasi editor; non serve alcun
comando.

Esempio di file:

```
# Congela il firefox installato; mantieni la build attuale.
mozilla-firefox

# Blocca ogni pacchetto xlibre, qualsiasi versione.
xlibre-.*

# Trattieni l'intera serie KDE.
kde/

# Solo la build alienbob di discover.
@alienbob discover

# Solo repo ufficiale: non far sostituire mai questi dall'albero ufficiale.
@slackware ^mozilla-firefox-
@slackware kde/
```

Il comando `frozen` scrive esattamente le stesse righe, quindi i due metodi sono
intercambiabili. La modifica a mano è il modo per rimuovere o cambiare una
regola.

## Catalogo delle regole

| Regola che scrivi              | Cosa congela / nasconde                                             |
|--------------------------------|--------------------------------------------------------------------|
| `vlc`                          | ogni pacchetto il cui id contiene `vlc` (vlc e vlc-plugin-*)        |
| `^vlc-[0-9]`                   | solo il pacchetto `vlc`                                             |
| `mozilla-firefox`              | ogni id che contiene `mozilla-firefox`                             |
| `xf86-.*-202.*`                | pacchetti xf86 con versione `202x`                                 |
| `xlibre-.*`                    | ogni pacchetto `xlibre-*`                                          |
| `kde/`                         | l'intera serie `kde`, tutti i repo                                 |
| `ap/`                          | l'intera serie `ap`, tutti i repo                                  |
| `@alienbob vlc`                | `vlc` solo quando proviene dal repo `alienbob`                     |
| `@conraid kde/`                | la serie `kde` solo dal repo `conraid`                            |
| `@slackware mozilla-firefox`   | `mozilla-firefox` solo dal repo ufficiale                         |
| `@slackware kde/`              | la serie `kde` solo dal repo ufficiale                           |

## Elencare e rimuovere

- Non esiste un comando per elencare; leggi direttamente il file:

  ```
  cat /etc/slacker/blacklist
  ```

  Puoi anche verificare l'effetto: un pacchetto in blacklist ma non installato
  compare ancora in `slacker search NOME` e `slacker info NOME` con
  `[blacklisted]`.

- Non esiste un comando per "scongelare". Per rimuovere o cambiare una regola,
  modifica `/etc/slacker/blacklist` ed elimina o cambia la riga.

## Note

- `frozen` aggiunge soltanto; non rimuove mai. Aggiungere di nuovo una regola
  esistente non fa nulla (viene segnalata come già presente).
- La regex è confrontata con l'id completo `nome-versione-arch-build`, non solo
  con il nome. Usa ancore come `^nome-` quando devi essere preciso.
- Lo scope `@repo` usa il nome del repository dal tuo file `repos`. Controlla i
  nomi nell'elenco che `frozen` mostra quando una regola nomina un repo
  sconosciuto, oppure leggi `/etc/slacker/repos`.
- Scrivere la blacklist richiede root, perché il file si trova sotto
  `/etc/slacker`.

[Torna su](#top)

---

<a id="ru"></a>
# Русский

Как добавлять пакеты и шаблоны в чёрный список (blacklist) slacker — как командой
`frozen`, так и редактированием файла вручную. Это полный каталог форм правил и
команд, которые их создают.

## Что делает чёрный список

Правило чёрного списка выбирает пакеты. Эффект зависит от того, установлен ли
выбранный пакет:

- Установлен и попал под правило: пакет **заморожен** (frozen). slacker никогда
  не устанавливает, не обновляет, не переустанавливает и не удаляет его, и он
  никогда не появляется в `clean-system`.
- Не установлен и попал под правило: пакет **скрыт** из `install-new`, из
  обновлений и из `check-updates`. Он по-прежнему виден в `search` и `info` с
  пометкой `[blacklisted]`.

## Как уберечь пакет от clean-system

Заморозка пакета чёрным списком — **один из трёх** способов уберечь его от
`clean-system`, который удаляет пакеты, отсутствующие в официальной базе
(официальный `PACKAGES.TXT` плюс любой репозиторий, помеченный `immutable`):

- правило **чёрного списка** — для отдельного пакета (этот файл);
- его **build-тег** в `IGNORE_TAGS` (в `slacker.conf`), напр. `_SBo cf alien`;
- его репозиторий, помеченный **`immutable`** в `repos`, — это сохраняет все
  пакеты, относимые к этому репозиторию.

Для целого subtree Slackware (`extra/`, `testing/`, `patches/`) лучше пометить
репозиторий как `immutable`, чем вносить каждый пакет в чёрный список вручную.

## Синтаксис правил

Каждая запись — это одно правило:

```
[@repo] PATTERN
```

- `PATTERN`, оканчивающийся на `/`, — правило **серии** (например, `kde/`); оно
  выбирает серию Slackware.
- Любой другой `PATTERN` — это незаякоренное **регулярное выражение**,
  сопоставляемое с полным идентификатором пакета `имя-версия-arch-build` (как в
  slackpkg).
- Необязательный префикс `@repo` ограничивает правило одним репозиторием. Для
  доступного пакета это его репозиторий-кандидат; для установленного — тот
  репозиторий, откуда он пришёл.

Поскольку регулярное выражение сопоставляется с полным id и не заякорено:

- простое `vlc` выбирает `vlc`, а также `vlc-plugin-*` (подстрока),
- `^vlc-[0-9]` выбирает только пакет `vlc`,
- `xf86-.*-202.*` выбирает сборки с версией `202x`,
- используйте `^` и `$` для точного совпадения.

Официальный репозиторий — это обычный репозиторий с именем. Используйте это имя
с `@`. В конфигурации по умолчанию он называется `slackware`, поэтому правила
только для официального выглядят так: `@slackware PATTERN`.

## Способ 1: команда `frozen`

```
slacker frozen ПРАВИЛО [ПРАВИЛО ...]
```

Каждый аргумент — одно правило. Команда проверяет каждое правило, показывает, что
оно заморозит, запрашивает подтверждение, затем добавляет новые правила в
`/etc/slacker/blacklist`. Требуются права root.

### Кавычки

Кавычки нужны командной оболочке, а не slacker. Используйте двойные кавычки,
когда правило:

- содержит пробел (любое правило вида `@repo PATTERN`), или
- содержит спецсимвол оболочки: `*` `?` `[` `]` `(` `)` `|` `$`.

Простые имена и `серия/` кавычек не требуют. Если сомневаетесь — заключайте в
кавычки: двойные кавычки никогда не вредят.

```
slacker frozen vlc                 # без кавычек нормально
slacker frozen kde/                # без кавычек нормально
slacker frozen "xlibre-*"          # ОБЯЗАТЕЛЬНО: * — это glob оболочки
slacker frozen "@alienbob vlc"     # ОБЯЗАТЕЛЬНО: содержит пробел
slacker frozen "@alienbob vlc-*"   # ОБЯЗАТЕЛЬНО: пробел (@repo) И glob *
```

Что на самом деле делает `"@alienbob vlc-*"`: `@alienbob` ограничивает правило
репозиторием `alienbob`, а `vlc-*` сопоставляется как **незаякоренное регулярное
выражение** с полным id — в регулярном выражении `-*` означает «ноль или более
дефисов», а не «что угодно». Поэтому оно замораживает любой пакет `alienbob`, чей
id **содержит** `vlc` (напр. `vlc`, `vlc-plugin-qt`), ровно как простое `vlc`;
`-*` ничего не добавляет. Чтобы заморозить только `vlc`, заякорите:
`"@alienbob ^vlc-[0-9]"` (для «vlc, за которым что угодно» регекс — `vlc-.*`, а не
`vlc-*`).

### Заморозка пакета против карантина репозитория

Blacklist замораживает отдельные **пакеты**. Воздействие на целый **репозиторий**
— отдельный механизм, *карантин*: `distrust-repo` помещает репозиторий в карантин
вручную, `vet-repo` перепроверяет его по запросу, `trust-repo` снимает карантин,
который вы считаете ложным срабатыванием. slacker также автоматически помещает в
карантин репозиторий, не прошедший проверку (недоступный или отдающий
искажённые/враждебные метаданные); репозиторий в карантине не предоставляет
пакетов. Правило: blacklist — для пакетов, карантин — для репозиториев.

### Одно или несколько правил

```
slacker frozen vlc
slacker frozen vlc "xf86-.*-202.*" "kde/" "@alienbob discover"
```

### Формы

```
# один пакет (внимание: выбирает и имена, содержащие эту строку)
slacker frozen mozilla-firefox

# строго только этот пакет
slacker frozen "^mozilla-firefox-[0-9]"

# регулярное выражение по всем репозиториям
slacker frozen "xf86-.*-202.*"
slacker frozen "xlibre-.*"

# целая серия (все репозитории)
slacker frozen kde/

# ограничить одним репозиторием: пакет из конкретного репозитория
slacker frozen "@alienbob discover"

# ограничить одним репозиторием: целая серия из конкретного репозитория
slacker frozen "@conraid kde/"

# только официальный репозиторий (имя по умолчанию: slackware)
slacker frozen "@slackware mozilla-firefox"
slacker frozen "@slackware kde/"
slacker frozen "@slackware ^mozilla-firefox-"
```

### Проверка и предупреждения

- Синтаксическая ошибка (например, `@repo` без шаблона) фатальна: ничего не
  записывается, и все проблемы выводятся вместе, чтобы их можно было исправить
  за один проход.
- Правило, которое разбирается, но похоже на ошибку, помечается, и вас
  спрашивают, объявить ли его всё равно:
  - `@repo`, не соответствующий ни одному активному репозиторию (вероятная
    опечатка),
  - регулярное выражение, содержащее пробел; идентификаторы пакетов никогда не
    содержат пробелов, поэтому обычно это означает забытую `@` (например,
    `conraid foo` вместо `@conraid foo`) или ошибку с кавычками.
- Правила, уже присутствующие в файле, пропускаются, и число в подтверждении
  отражает только реально добавляемые правила.

### Флаги

- `--yes` (или `-y`): пропустить предупреждения и подтверждение.
- `--config-dir DIR`: использовать blacklist в каталоге `DIR` вместо
  `/etc/slacker`.

## Способ 2: редактирование файла вручную

Чёрный список — это простой текстовый файл по пути:

```
/etc/slacker/blacklist
```

Одно правило на строку. Пустые строки игнорируются. `#` начинает комментарий
(всю строку или в конце строки). Редактируйте любым редактором; команда не
нужна.

Пример файла:

```
# Заморозить установленный firefox; сохранить текущую сборку.
mozilla-firefox

# Закрепить любой пакет xlibre, любой версии.
xlibre-.*

# Удержать всю серию KDE.
kde/

# Только сборку discover из alienbob.
@alienbob discover

# Только официальный репозиторий: никогда не давать официальному дереву их заменить.
@slackware ^mozilla-firefox-
@slackware kde/
```

Команда `frozen` записывает ровно те же строки, поэтому оба способа
взаимозаменяемы. Ручное редактирование — это способ удалить или изменить
правило.

## Каталог правил

| Правило, которое вы пишете     | Что замораживает / скрывает                                        |
|--------------------------------|--------------------------------------------------------------------|
| `vlc`                          | любой пакет, чей id содержит `vlc` (vlc и vlc-plugin-*)             |
| `^vlc-[0-9]`                   | только пакет `vlc`                                                 |
| `mozilla-firefox`              | любой id, содержащий `mozilla-firefox`                            |
| `xf86-.*-202.*`                | пакеты xf86 с версией `202x`                                      |
| `xlibre-.*`                    | любой пакет `xlibre-*`                                            |
| `kde/`                         | всю серию `kde`, все репозитории                                  |
| `ap/`                          | всю серию `ap`, все репозитории                                   |
| `@alienbob vlc`                | `vlc` только когда он из репозитория `alienbob`                   |
| `@conraid kde/`                | серию `kde` только из репозитория `conraid`                      |
| `@slackware mozilla-firefox`   | `mozilla-firefox` только из официального репозитория             |
| `@slackware kde/`              | серию `kde` только из официального репозитория                  |

## Просмотр и удаление

- Отдельной команды для просмотра нет; читайте файл напрямую:

  ```
  cat /etc/slacker/blacklist
  ```

  Эффект можно проверить: пакет из чёрного списка, но не установленный,
  по-прежнему появляется в `slacker search ИМЯ` и `slacker info ИМЯ` с пометкой
  `[blacklisted]`.

- Команды «разморозить» нет. Чтобы удалить или изменить правило, отредактируйте
  `/etc/slacker/blacklist` и удалите или измените строку.

## Примечания

- `frozen` только добавляет; он никогда не удаляет. Повторное добавление
  существующего правила ничего не делает (оно отмечается как уже присутствующее).
- Регулярное выражение сопоставляется с полным id `имя-версия-arch-build`, а не
  только с именем. Используйте якоря вроде `^имя-`, когда нужна точность.
- Область `@repo` использует имя репозитория из вашего файла `repos`. Проверьте
  имена в списке, который `frozen` показывает, когда правило называет неизвестный
  репозиторий, или читайте `/etc/slacker/repos`.
- Запись чёрного списка требует root, потому что файл находится в
  `/etc/slacker`.

[Наверх](#top)

---

<a id="es"></a>
# Español

Cómo añadir paquetes y patrones a la lista negra (blacklist) de slacker, tanto
con el comando `frozen` como editando el archivo a mano. Este es un catálogo
completo de las formas de las reglas y de los comandos que las crean.

## Qué hace la lista negra

Una regla de la lista negra selecciona paquetes. El efecto depende de si el
paquete seleccionado está instalado:

- Instalado y seleccionado: el paquete queda **congelado** (frozen). slacker
  nunca lo instala, actualiza, reinstala ni elimina, y nunca aparece en
  `clean-system`.
- No instalado y seleccionado: el paquete queda **oculto** en `install-new`, en
  las actualizaciones y en `check-updates`. Sigue apareciendo en `search` e
  `info`, marcado con `[blacklisted]`.

## Mantener un paquete fuera de clean-system

Congelar un paquete con la lista negra es **una de tres** formas de mantenerlo
fuera de `clean-system`, que elimina los paquetes que ya no están en la base
oficial (el `PACKAGES.TXT` oficial más cualquier repositorio marcado
`immutable`):

- una regla de **lista negra** — por paquete (este archivo);
- su **build tag** en `IGNORE_TAGS` (en `slacker.conf`), p. ej. `_SBo cf alien`;
- su repositorio marcado **`immutable`** en `repos`, que conserva todos los
  paquetes atribuidos a ese repositorio.

Para todo un subtree de Slackware (`extra/`, `testing/`, `patches/`) es
preferible marcar el repositorio como `immutable` en lugar de poner cada paquete
en la lista negra a mano.

## Sintaxis de las reglas

Cada entrada es una regla:

```
[@repo] PATTERN
```

- Un `PATTERN` que termina en `/` es una regla de **serie** (p. ej. `kde/`);
  selecciona una serie de Slackware.
- Cualquier otro `PATTERN` es una **expresión regular** sin anclar, comparada con
  el id completo del paquete `nombre-versión-arch-build` (estilo slackpkg).
- El prefijo opcional `@repo` limita la regla a un solo repositorio. Para un
  paquete disponible es su repositorio candidato; para uno instalado es el
  repositorio del que proviene.

Como la regex se compara con el id completo y no está anclada:

- un simple `vlc` selecciona `vlc` y también `vlc-plugin-*` (subcadena),
- `^vlc-[0-9]` selecciona solo el paquete `vlc`,
- `xf86-.*-202.*` selecciona las builds con versión `202x`,
- usa `^` y `$` para una coincidencia exacta.

El repositorio oficial es simplemente un repositorio con nombre. Usa ese nombre
con `@`. En una configuración por defecto se llama `slackware`, así que las
reglas solo-oficiales son `@slackware PATTERN`.

## Método 1: el comando `frozen`

```
slacker frozen REGLA [REGLA ...]
```

Cada argumento es una regla. El comando valida cada regla, muestra qué va a
congelar, pide confirmación y luego añade las nuevas reglas a
`/etc/slacker/blacklist`. Requiere permisos de root.

### Comillas

Las comillas son para la shell, no para slacker. Usa comillas dobles cuando una
regla:

- contiene un espacio (cualquier regla `@repo PATTERN`), o
- contiene un carácter especial de la shell: `*` `?` `[` `]` `(` `)` `|` `$`.

Los nombres simples y `serie/` no necesitan comillas. En caso de duda, ponlas:
las comillas dobles nunca estorban.

```
slacker frozen vlc                 # bien sin comillas
slacker frozen kde/                # bien sin comillas
slacker frozen "xlibre-*"          # OBLIGATORIAS: * es un glob de la shell
slacker frozen "@alienbob vlc"     # OBLIGATORIAS: contiene un espacio
slacker frozen "@alienbob vlc-*"   # OBLIGATORIAS: un espacio (@repo) Y un glob *
```

Qué hace realmente `"@alienbob vlc-*"`: `@alienbob` limita la regla al repo
`alienbob`, y `vlc-*` se compara como **regex sin anclar** contra el id completo —
en una regex `-*` significa "cero o más guiones", no "cualquier cosa". Así que
congela cualquier paquete `alienbob` cuyo id **contenga** `vlc` (p. ej. `vlc`,
`vlc-plugin-qt`), igual que un simple `vlc`; el `-*` no añade nada. Para congelar
solo `vlc`, ánclala: `"@alienbob ^vlc-[0-9]"` (para "vlc seguido de cualquier
cosa" la regex es `vlc-.*`, no `vlc-*`).

### Congelar un paquete vs. poner en cuarentena un repo

La blacklist congela paquetes **individuales**. Actuar sobre un **repositorio**
entero es un mecanismo aparte — la *cuarentena*: `distrust-repo` pone en cuarentena
un repo tú mismo, `vet-repo` lo revisa de nuevo bajo demanda, `trust-repo` levanta
una cuarentena que juzgues un falso positivo. slacker también pone en cuarentena
automáticamente un repo que falla la verificación (inalcanzable o que sirve
metadatos malformados/hostiles); un repo en cuarentena no provee ningún paquete.
Regla práctica: blacklist para paquetes, cuarentena para repositorios.

### Una o varias reglas

```
slacker frozen vlc
slacker frozen vlc "xf86-.*-202.*" "kde/" "@alienbob discover"
```

### Formas

```
# un solo paquete (ojo: también selecciona nombres que lo contienen)
slacker frozen mozilla-firefox

# solo ese paquete, de forma estricta
slacker frozen "^mozilla-firefox-[0-9]"

# regex en todos los repositorios
slacker frozen "xf86-.*-202.*"
slacker frozen "xlibre-.*"

# una serie entera (todos los repositorios)
slacker frozen kde/

# limitar a un repositorio: un paquete de un repositorio concreto
slacker frozen "@alienbob discover"

# limitar a un repositorio: una serie entera de un repositorio concreto
slacker frozen "@conraid kde/"

# solo repositorio oficial (nombre por defecto: slackware)
slacker frozen "@slackware mozilla-firefox"
slacker frozen "@slackware kde/"
slacker frozen "@slackware ^mozilla-firefox-"
```

### Validación y avisos

- Un error de sintaxis (por ejemplo `@repo` sin patrón) es fatal: no se escribe
  nada y todos los problemas se listan juntos, para corregirlos de una sola vez.
- Una regla que es válida pero parece un error se señala y se te pregunta si
  declararla de todos modos:
  - un `@repo` que no corresponde a ningún repositorio activo (probable errata),
  - una regex que contiene un espacio; los id de paquete nunca contienen
    espacios, así que normalmente significa una `@` olvidada (por ejemplo
    `conraid foo` en vez de `@conraid foo`) o un error de comillas.
- Las reglas ya presentes en el archivo se omiten, y el número en la
  confirmación refleja solo las reglas realmente añadidas.

### Opciones

- `--yes` (o `-y`): omitir los avisos y la confirmación.
- `--config-dir DIR`: usar una blacklist bajo `DIR` en vez de `/etc/slacker`.

## Método 2: editar el archivo a mano

La lista negra es un archivo de texto plano en:

```
/etc/slacker/blacklist
```

Una regla por línea. Las líneas vacías se ignoran. Un `#` inicia un comentario
(línea entera o final). Edítalo con cualquier editor; no hace falta ningún
comando.

Ejemplo de archivo:

```
# Congela el firefox instalado; mantén la build actual.
mozilla-firefox

# Fija cualquier paquete xlibre, cualquier versión.
xlibre-.*

# Retén toda la serie KDE.
kde/

# Solo la build de alienbob de discover.
@alienbob discover

# Solo repositorio oficial: que el árbol oficial nunca los sustituya.
@slackware ^mozilla-firefox-
@slackware kde/
```

El comando `frozen` escribe exactamente las mismas líneas, así que ambos métodos
son intercambiables. La edición a mano es la forma de eliminar o cambiar una
regla.

## Catálogo de reglas

| Regla que escribes             | Qué congela / oculta                                               |
|--------------------------------|--------------------------------------------------------------------|
| `vlc`                          | cualquier paquete cuyo id contenga `vlc` (vlc y vlc-plugin-*)       |
| `^vlc-[0-9]`                   | solo el paquete `vlc`                                              |
| `mozilla-firefox`              | cualquier id que contenga `mozilla-firefox`                       |
| `xf86-.*-202.*`                | paquetes xf86 con versión `202x`                                  |
| `xlibre-.*`                    | cualquier paquete `xlibre-*`                                       |
| `kde/`                         | toda la serie `kde`, todos los repositorios                       |
| `ap/`                          | toda la serie `ap`, todos los repositorios                        |
| `@alienbob vlc`                | `vlc` solo cuando proviene del repositorio `alienbob`             |
| `@conraid kde/`                | la serie `kde` solo del repositorio `conraid`                    |
| `@slackware mozilla-firefox`   | `mozilla-firefox` solo del repositorio oficial                   |
| `@slackware kde/`              | la serie `kde` solo del repositorio oficial                     |

## Listar y eliminar

- No hay un comando para listar; lee el archivo directamente:

  ```
  cat /etc/slacker/blacklist
  ```

  También puedes comprobar el efecto: un paquete en la lista negra pero no
  instalado sigue apareciendo en `slacker search NOMBRE` e `slacker info NOMBRE`
  marcado con `[blacklisted]`.

- No hay un comando para "descongelar". Para eliminar o cambiar una regla, edita
  `/etc/slacker/blacklist` y borra o modifica la línea.

## Notas

- `frozen` solo añade; nunca elimina. Volver a añadir una regla existente no hace
  nada (se informa como ya presente).
- La regex se compara con el id completo `nombre-versión-arch-build`, no solo con
  el nombre. Usa anclas como `^nombre-` cuando necesites precisión.
- El ámbito `@repo` usa el nombre del repositorio de tu archivo `repos`. Revisa
  los nombres en la lista que `frozen` muestra cuando una regla nombra un
  repositorio desconocido, o lee `/etc/slacker/repos`.
- Escribir la lista negra requiere root, porque el archivo está bajo
  `/etc/slacker`.

[Volver arriba](#top)

---

<a id="zh"></a>
# 中文

如何将软件包和模式添加到 slacker 的黑名单（blacklist），既可以用 `frozen` 命令，也可以
手动编辑文件。以下是规则形式以及创建它们的命令的完整目录。

## 黑名单的作用

一条黑名单规则会匹配软件包。其效果取决于被匹配的包是否已安装：

- 已安装且被匹配：该包被**冻结**（frozen）。slacker 永远不会安装、升级、重新安装或
  删除它，并且它绝不会出现在 `clean-system` 中。
- 未安装且被匹配：该包在 `install-new`、升级以及 `check-updates` 中被**隐藏**。它仍会
  出现在 `search` 和 `info` 中，并标记为 `[blacklisted]`。

## 让软件包不被 clean-system 删除

用黑名单冻结软件包，是让它**不被 `clean-system` 删除的三种方法之一**。`clean-system`
会删除不在官方基准中的软件包（官方 `PACKAGES.TXT` 加上任何标记为 `immutable` 的仓库）：

- 一条**黑名单**规则 —— 针对单个软件包（本文件）；
- 把它的 **build tag** 写入 `IGNORE_TAGS`（在 `slacker.conf` 中），例如 `_SBo cf alien`；
- 把它所属的仓库在 `repos` 中标记为 **`immutable`**，这样归属该仓库的所有软件包都会被保留。

对于整个 Slackware 子树仓库（`extra/`、`testing/`、`patches/`），建议把该仓库标记为
`immutable`，而不是逐个把软件包加入黑名单。

## 规则语法

每一条记录就是一条规则：

```
[@repo] PATTERN
```

- 以 `/` 结尾的 `PATTERN` 是**系列（series）**规则（例如 `kde/`），它匹配一个 Slackware
  系列。
- 其他任何 `PATTERN` 都是**未锚定的正则表达式**，与完整的软件包 id
  `名称-版本-架构-构建号`（slackpkg 风格）进行匹配。
- 可选的 `@repo` 前缀将规则限定到单个仓库。对于可用包，它指候选仓库；对于已安装包，它
  指该包的来源仓库。

由于正则表达式是对完整 id 进行匹配且未锚定：

- 单独的 `vlc` 会匹配 `vlc`，也会匹配 `vlc-plugin-*`（子串），
- `^vlc-[0-9]` 只匹配 `vlc` 这个包，
- `xf86-.*-202.*` 匹配版本为 `202x` 的构建，
- 使用 `^` 和 `$` 进行精确匹配。

官方仓库只是一个有名字的普通仓库。用 `@` 加上那个名字。在默认配置中它名为
`slackware`，所以“仅官方”的规则写作 `@slackware PATTERN`。

## 方法一：`frozen` 命令

```
slacker frozen 规则 [规则 ...]
```

每个参数是一条规则。该命令会校验每条规则，显示它将冻结什么，请求确认，然后把新规则
追加到 `/etc/slacker/blacklist`。需要 root 权限。

### 引号

引号是给 shell 用的，不是给 slacker 用的。当规则满足以下情况时，请使用双引号：

- 包含空格（任何 `@repo PATTERN` 规则），或
- 包含 shell 特殊字符：`*` `?` `[` `]` `(` `)` `|` `$`。

普通名称和 `系列/` 不需要引号。拿不准时就加引号：双引号绝不会有害。

```
slacker frozen vlc                 # 不加引号也可以
slacker frozen kde/                # 不加引号也可以
slacker frozen "xlibre-*"          # 必须加：* 是 shell 通配符
slacker frozen "@alienbob vlc"     # 必须加：包含空格
slacker frozen "@alienbob vlc-*"   # 必须加：空格（@repo）以及 * 通配符
```

`"@alienbob vlc-*"` 实际做什么：`@alienbob` 把规则限定到 `alienbob` 仓库，而 `vlc-*`
是按**未锚定的正则**匹配完整 id 的——在正则里 `-*` 表示“零个或多个连字符”，而非“任意
内容”。因此它会冻结任何 id **含** `vlc` 的 `alienbob` 包（如 `vlc`、`vlc-plugin-qt`），
与单写 `vlc` 一样；`-*` 没有增加任何效果。若只想冻结 `vlc`，请加锚点：
`"@alienbob ^vlc-[0-9]"`（“vlc 后接任意内容”的正则是 `vlc-.*`，而不是 `vlc-*`）。

### 冻结软件包 vs. 隔离仓库

blacklist 冻结的是单个**软件包**。对整个**仓库**采取行动是另一套机制——*隔离*：
`distrust-repo` 由你自己把某仓库放入隔离，`vet-repo` 按需重新检查它，`trust-repo`
解除你判断为误报的隔离。slacker 也会自动隔离未通过审查的仓库（无法访问，或提供畸形/
恶意元数据）；被隔离的仓库不提供任何软件包。经验法则：blacklist 用于软件包，隔离用于
仓库。

### 一条或多条规则

```
slacker frozen vlc
slacker frozen vlc "xf86-.*-202.*" "kde/" "@alienbob discover"
```

### 各种形式

```
# 单个软件包（注意：也会匹配包含该字符串的名称）
slacker frozen mozilla-firefox

# 严格地只匹配该包
slacker frozen "^mozilla-firefox-[0-9]"

# 跨所有仓库的正则
slacker frozen "xf86-.*-202.*"
slacker frozen "xlibre-.*"

# 整个系列（所有仓库）
slacker frozen kde/

# 限定到某个仓库：来自特定仓库的某个包
slacker frozen "@alienbob discover"

# 限定到某个仓库：来自特定仓库的整个系列
slacker frozen "@conraid kde/"

# 仅官方仓库（默认名称：slackware）
slacker frozen "@slackware mozilla-firefox"
slacker frozen "@slackware kde/"
slacker frozen "@slackware ^mozilla-firefox-"
```

### 校验与警告

- 语法错误（例如 `@repo` 后面没有模式）是致命的：不会写入任何内容，并且所有问题会一起
  列出，以便一次性修正。
- 能够解析但看起来像错误的规则会被标记，并询问你是否仍要声明它：
  - `@repo` 没有对应任何活动仓库（很可能是拼写错误），
  - 含有空格的正则；软件包 id 从不包含空格，所以这通常意味着漏写了 `@`（例如
    `conraid foo` 而不是 `@conraid foo`）或引号用错。
- 文件中已存在的规则会被跳过，确认时显示的数量只反映实际新增的规则。

### 选项

- `--yes`（或 `-y`）：跳过警告和确认。
- `--config-dir DIR`：使用 `DIR` 下的黑名单，而不是 `/etc/slacker`。

## 方法二：手动编辑文件

黑名单是一个纯文本文件，位于：

```
/etc/slacker/blacklist
```

每行一条规则。空行被忽略。`#` 开始一个注释（整行或行尾）。用任意编辑器编辑即可，不需要
任何命令。

文件示例：

```
# 冻结已安装的 firefox；保留当前构建。
mozilla-firefox

# 固定任意版本的所有 xlibre 包。
xlibre-.*

# 保持整个 KDE 系列不变。
kde/

# 只冻结来自 alienbob 的 discover 构建。
@alienbob discover

# 仅官方仓库：永远不让官方树替换这些。
@slackware ^mozilla-firefox-
@slackware kde/
```

`frozen` 命令写入的就是完全相同的行，因此两种方法可以互换。手动编辑是删除或修改规则的
方式。

## 规则目录

| 你写的规则                     | 它冻结 / 隐藏什么                                                 |
|--------------------------------|------------------------------------------------------------------|
| `vlc`                          | id 中含有 `vlc` 的任何包（vlc 和 vlc-plugin-*）                   |
| `^vlc-[0-9]`                   | 只有 `vlc` 这个包                                                |
| `mozilla-firefox`              | id 中含有 `mozilla-firefox` 的任何包                            |
| `xf86-.*-202.*`                | 版本为 `202x` 的 xf86 包                                        |
| `xlibre-.*`                    | 任何 `xlibre-*` 包                                              |
| `kde/`                         | 整个 `kde` 系列，所有仓库                                       |
| `ap/`                          | 整个 `ap` 系列，所有仓库                                        |
| `@alienbob vlc`                | 仅当 `vlc` 来自 `alienbob` 仓库时                              |
| `@conraid kde/`                | 仅来自 `conraid` 仓库的 `kde` 系列                            |
| `@slackware mozilla-firefox`   | 仅来自官方仓库的 `mozilla-firefox`                            |
| `@slackware kde/`              | 仅来自官方仓库的 `kde` 系列                                   |

## 查看与删除

- 没有单独的“列出”命令；直接读取文件：

  ```
  cat /etc/slacker/blacklist
  ```

  你也可以验证效果：一个在黑名单中但未安装的包，仍会在 `slacker search 名称` 和
  `slacker info 名称` 中出现，并标记为 `[blacklisted]`。

- 没有“解冻”命令。要删除或修改一条规则，请编辑 `/etc/slacker/blacklist` 并删除或修改
  对应行。

## 注意事项

- `frozen` 只追加，从不删除。再次添加已存在的规则不会有任何效果（会被报告为已存在）。
- 正则表达式是对完整 id `名称-版本-架构-构建号` 进行匹配，而不仅仅是名称。需要精确时请
  使用 `^名称-` 这样的锚点。
- `@repo` 作用域使用你的 `repos` 文件中的仓库名称。当某条规则指向未知仓库时，可在
  `frozen` 显示的列表中查看名称，或阅读 `/etc/slacker/repos`。
- 写入黑名单需要 root 权限，因为该文件位于 `/etc/slacker` 下。

[返回顶部](#top)

---

<a id="fr"></a>
# Français

Comment ajouter des paquets et des motifs à la liste noire (blacklist) de
slacker, à la fois avec la commande `frozen` et en éditant le fichier à la main.
Voici un catalogue complet des formes de règles et des commandes qui les créent.

## Ce que fait la liste noire

Une règle de liste noire sélectionne des paquets. L'effet dépend du fait que le
paquet sélectionné soit installé ou non :

- Installé et sélectionné : le paquet est **gelé** (frozen). slacker ne
  l'installe, ne le met à jour, ne le réinstalle ni ne le supprime jamais, et il
  n'apparaît jamais dans `clean-system`.
- Non installé et sélectionné : le paquet est **masqué** dans `install-new`,
  dans les mises à jour et dans `check-updates`. Il apparaît toujours dans
  `search` et `info`, marqué `[blacklisted]`.

## Garder un paquet hors de clean-system

Geler un paquet avec la blacklist est **l'une des trois** façons de le garder
hors de `clean-system`, qui supprime les paquets ne faisant plus partie de la
base officielle (le `PACKAGES.TXT` officiel plus tout dépôt marqué `immutable`) :

- une règle de **blacklist** — par paquet (ce fichier) ;
- son **build tag** dans `IGNORE_TAGS` (dans `slacker.conf`), p. ex. `_SBo cf alien` ;
- son dépôt marqué **`immutable`** dans `repos`, ce qui conserve tous les paquets
  attribués à ce dépôt.

Pour tout un sous-arbre Slackware (`extra/`, `testing/`, `patches/`), préférez
marquer le dépôt comme `immutable` plutôt que de mettre chaque paquet en
blacklist à la main.

## Syntaxe des règles

Chaque entrée est une règle :

```
[@repo] PATTERN
```

- Un `PATTERN` qui se termine par `/` est une règle de **série** (par ex.
  `kde/`) ; elle sélectionne une série Slackware.
- Tout autre `PATTERN` est une **expression régulière** non ancrée, comparée à
  l'id complet du paquet `nom-version-arch-build` (style slackpkg).
- Le préfixe facultatif `@repo` limite la règle à un seul dépôt. Pour un paquet
  disponible, c'est son dépôt candidat ; pour un paquet installé, c'est le dépôt
  d'où il provient.

Comme la regex est comparée à l'id entier et n'est pas ancrée :

- un simple `vlc` sélectionne `vlc` ainsi que `vlc-plugin-*` (sous-chaîne),
- `^vlc-[0-9]` ne sélectionne que le paquet `vlc`,
- `xf86-.*-202.*` sélectionne les builds dont la version est `202x`,
- utilisez `^` et `$` pour une correspondance exacte.

Le dépôt officiel est simplement un dépôt avec un nom. Utilisez ce nom avec `@`.
Dans une configuration par défaut, il s'appelle `slackware`, donc les règles
réservées à l'officiel s'écrivent `@slackware PATTERN`.

## Méthode 1 : la commande `frozen`

```
slacker frozen RÈGLE [RÈGLE ...]
```

Chaque argument est une règle. La commande valide chaque règle, montre ce
qu'elle va geler, demande confirmation, puis ajoute les nouvelles règles à
`/etc/slacker/blacklist`. Elle nécessite les droits root.

### Guillemets

Les guillemets servent au shell, pas à slacker. Utilisez des guillemets doubles
lorsqu'une règle :

- contient une espace (toute règle `@repo PATTERN`), ou
- contient un caractère spécial du shell : `*` `?` `[` `]` `(` `)` `|` `$`.

Les noms simples et `série/` n'ont pas besoin de guillemets. En cas de doute,
mettez-en : les guillemets doubles ne gênent jamais.

```
slacker frozen vlc                 # correct sans guillemets
slacker frozen kde/                # correct sans guillemets
slacker frozen "xlibre-*"          # OBLIGATOIRES : * est un glob du shell
slacker frozen "@alienbob vlc"     # OBLIGATOIRES : contient une espace
slacker frozen "@alienbob vlc-*"   # OBLIGATOIRES : une espace (@repo) ET un glob *
```

Ce que fait réellement `"@alienbob vlc-*"` : `@alienbob` restreint la règle au dépôt
`alienbob`, et `vlc-*` est comparé comme une **regex non ancrée** à l'id complet —
dans une regex `-*` veut dire « zéro ou plusieurs tirets », pas « n'importe quoi ».
Elle gèle donc tout paquet `alienbob` dont l'id **contient** `vlc` (p. ex. `vlc`,
`vlc-plugin-qt`), exactement comme un simple `vlc` ; le `-*` n'ajoute rien. Pour ne
geler que `vlc`, ancre-la : `"@alienbob ^vlc-[0-9]"` (pour « vlc suivi de n'importe
quoi », la regex est `vlc-.*`, pas `vlc-*`).

### Geler un paquet vs. mettre en quarantaine un dépôt

La blacklist gèle des **paquets** individuels. Agir sur un **dépôt** entier est un
mécanisme distinct — la *quarantaine* : `distrust-repo` met un dépôt en quarantaine
toi-même, `vet-repo` le recontrôle à la demande, `trust-repo` lève une quarantaine
que tu juges être un faux positif. slacker met aussi automatiquement en quarantaine
un dépôt qui échoue au contrôle (injoignable ou servant des métadonnées
malformées/hostiles) ; un dépôt en quarantaine ne fournit aucun paquet. Règle
pratique : la blacklist pour les paquets, la quarantaine pour les dépôts.

### Une ou plusieurs règles

```
slacker frozen vlc
slacker frozen vlc "xf86-.*-202.*" "kde/" "@alienbob discover"
```

### Formes

```
# un seul paquet (attention : sélectionne aussi les noms qui le contiennent)
slacker frozen mozilla-firefox

# uniquement ce paquet, de façon stricte
slacker frozen "^mozilla-firefox-[0-9]"

# regex sur tous les dépôts
slacker frozen "xf86-.*-202.*"
slacker frozen "xlibre-.*"

# une série entière (tous les dépôts)
slacker frozen kde/

# limiter à un dépôt : un paquet d'un dépôt précis
slacker frozen "@alienbob discover"

# limiter à un dépôt : une série entière d'un dépôt précis
slacker frozen "@conraid kde/"

# dépôt officiel uniquement (nom par défaut : slackware)
slacker frozen "@slackware mozilla-firefox"
slacker frozen "@slackware kde/"
slacker frozen "@slackware ^mozilla-firefox-"
```

### Validation et avertissements

- Une erreur de syntaxe (par exemple `@repo` sans motif) est fatale : rien n'est
  écrit, et tous les problèmes sont listés ensemble afin de les corriger en une
  seule passe.
- Une règle qui s'analyse mais ressemble à une erreur est signalée, et il vous
  est demandé si vous voulez la déclarer quand même :
  - un `@repo` ne correspondant à aucun dépôt actif (probable faute de frappe),
  - une regex contenant une espace ; les id de paquet ne contiennent jamais
    d'espace, donc cela signifie généralement un `@` oublié (par exemple
    `conraid foo` au lieu de `@conraid foo`) ou une erreur de guillemets.
- Les règles déjà présentes dans le fichier sont ignorées, et le nombre affiché
  à la confirmation ne reflète que les règles réellement ajoutées.

### Options

- `--yes` (ou `-y`) : ignorer les avertissements et la confirmation.
- `--config-dir DIR` : utiliser une blacklist sous `DIR` au lieu de
  `/etc/slacker`.

## Méthode 2 : éditer le fichier à la main

La liste noire est un fichier texte brut situé à :

```
/etc/slacker/blacklist
```

Une règle par ligne. Les lignes vides sont ignorées. Un `#` commence un
commentaire (ligne entière ou en fin de ligne). Éditez-le avec n'importe quel
éditeur ; aucune commande n'est nécessaire.

Exemple de fichier :

```
# Geler le firefox installé ; conserver la build actuelle.
mozilla-firefox

# Épingler tout paquet xlibre, quelle que soit la version.
xlibre-.*

# Retenir toute la série KDE.
kde/

# Seulement la build alienbob de discover.
@alienbob discover

# Dépôt officiel uniquement : ne jamais laisser l'arbre officiel les remplacer.
@slackware ^mozilla-firefox-
@slackware kde/
```

La commande `frozen` écrit exactement les mêmes lignes, donc les deux méthodes
sont interchangeables. L'édition à la main est le moyen de supprimer ou de
modifier une règle.

## Catalogue des règles

| Règle que vous écrivez         | Ce qu'elle gèle / masque                                          |
|--------------------------------|-------------------------------------------------------------------|
| `vlc`                          | tout paquet dont l'id contient `vlc` (vlc et vlc-plugin-*)         |
| `^vlc-[0-9]`                   | uniquement le paquet `vlc`                                        |
| `mozilla-firefox`              | tout id contenant `mozilla-firefox`                              |
| `xf86-.*-202.*`                | les paquets xf86 dont la version est `202x`                      |
| `xlibre-.*`                    | tout paquet `xlibre-*`                                            |
| `kde/`                         | toute la série `kde`, tous les dépôts                            |
| `ap/`                          | toute la série `ap`, tous les dépôts                             |
| `@alienbob vlc`                | `vlc` seulement quand il provient du dépôt `alienbob`            |
| `@conraid kde/`                | la série `kde` seulement depuis le dépôt `conraid`             |
| `@slackware mozilla-firefox`   | `mozilla-firefox` seulement depuis le dépôt officiel            |
| `@slackware kde/`              | la série `kde` seulement depuis le dépôt officiel              |

## Lister et supprimer

- Il n'y a pas de commande pour lister ; lisez le fichier directement :

  ```
  cat /etc/slacker/blacklist
  ```

  Vous pouvez aussi vérifier l'effet : un paquet en liste noire mais non
  installé apparaît toujours dans `slacker search NOM` et `slacker info NOM`,
  marqué `[blacklisted]`.

- Il n'y a pas de commande pour « dégeler ». Pour supprimer ou modifier une
  règle, éditez `/etc/slacker/blacklist` et supprimez ou modifiez la ligne.

## Remarques

- `frozen` ne fait qu'ajouter ; il ne supprime jamais. Ré-ajouter une règle
  existante ne fait rien (elle est signalée comme déjà présente).
- La regex est comparée à l'id complet `nom-version-arch-build`, pas seulement au
  nom. Utilisez des ancres comme `^nom-` lorsque vous devez être précis.
- La portée `@repo` utilise le nom du dépôt issu de votre fichier `repos`.
  Vérifiez les noms dans la liste que `frozen` affiche quand une règle nomme un
  dépôt inconnu, ou lisez `/etc/slacker/repos`.
- Écrire la liste noire nécessite root, car le fichier se trouve sous
  `/etc/slacker`.

[Retour en haut](#top)
