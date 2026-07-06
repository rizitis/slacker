# bash-completion for slacker (https://forge.slackware.nl/rizitis/slacker)
# Hand-written; keep the command/flag lists in sync with src/main.rs (clap Cmd)
# and man/slacker.8. See DEV/4F/CODE_MAP.txt for the command surface.

_slacker_commands='update search file-search info list-repos status install
upgrade reinstall remove revert-pkg download upgrade-all upgrade-dist
install-new clean-system clean-cache new-config check-updates show-changelog
history find-mirror generate-template install-template remove-template
delete-template frozen unfrozen pin unpin add-repo del-repo pri-repo add-tag
del-tag vet-repo trust-repo distrust-repo help'

# Global flags clap propagates to every subcommand.
_slacker_global_flags='--config-dir --yes -y --dry-run --no-deps --help -h --version -V'

# Resolve the config dir from the current command line (--config-dir DIR),
# defaulting to /etc/slacker.
_slacker_config_dir() {
    local i
    for ((i = 1; i < ${#COMP_WORDS[@]}; i++)); do
        if [[ ${COMP_WORDS[i]} == --config-dir && -n ${COMP_WORDS[i+1]} ]]; then
            printf '%s' "${COMP_WORDS[i+1]}"
            return
        fi
    done
    printf '/etc/slacker'
}

# Installed package NAMES, one per line. /var/adm/packages entries are
# name-version-arch-build; strip the last three dash fields to get the name.
_slacker_installed_names() {
    local db=/var/adm/packages
    [[ -d $db ]] || return
    # Skip leftover `-upgraded-<timestamp>` records (interrupted upgradepkg);
    # slacker itself skips them too (system.rs is_removed_record).
    command ls "$db" 2>/dev/null \
        | grep -Ev -- '-upgraded-[0-9]' \
        | sed -E 's/(-[^-]+){3}$//'
}

# Repo NAMES from ${config}/repos: binary-repo lines only (3rd field is a
# URL or `mirror`), 2nd field = name. Skips comments, blanks, tag lines.
_slacker_repo_names() {
    local repos="$1/repos"
    [[ -f $repos ]] || return
    awk '
        /^[[:space:]]*#/ { next }
        /^[[:space:]]*$/ { next }
        {
            url = $3
            if (url ~ /:\/\// || url == "mirror" || url ~ /^mirror\//)
                print $2
        }' "$repos"
}

# Known build tags, one per line, as they appear on disk (e.g. _SBo, cf,
# alien). From repos tag lines + installed packages' build field (leading
# digits stripped). Empty tag dropped.
_slacker_build_tags() {
    local repos="$1/repos" db=/var/adm/packages
    {
        if [[ -f $repos ]]; then
            awk '
                /^[[:space:]]*#/ { next }
                /^[[:space:]]*$/ { next }
                {
                    url = $3
                    if (!(url ~ /:\/\// || url == "mirror" || url ~ /^mirror\//) && NF >= 3)
                        print $3
                }' "$repos"
        fi
        if [[ -d $db ]]; then
            command ls "$db" 2>/dev/null \
                | grep -Ev -- '-upgraded-[0-9]' \
                | sed -E 's/.*-([^-]+)$/\1/; s/^[0-9]+//' \
                | grep -v '^$'
        fi
    } | sort -u
}

# Template names from ${config}/templates, `.template` suffix stripped.
_slacker_template_names() {
    local dir="$1/templates"
    [[ -d $dir ]] || return
    command ls "$dir" 2>/dev/null | sed -E 's/\.template$//'
}

_slacker_dispatch() {
    local config; config=$(_slacker_config_dir)

    # A --config-dir value is being typed: complete a directory.
    if [[ $prev == --config-dir ]]; then
        compopt -o dirnames 2>/dev/null
        COMPREPLY=()
        return
    fi

    # Flags first when the word looks like one.
    if [[ $cur == -* ]]; then
        local flags="$_slacker_global_flags"
        case $sub in
            history) flags+=' --last --since --installed --removed --upgraded' ;;
            download) flags+=' -o --output' ;;
        esac
        COMPREPLY=($(compgen -W "$flags" -- "$cur"))
        return
    fi

    # download -o/--output takes a directory.
    if [[ $sub == download && ( $prev == -o || $prev == --output ) ]]; then
        compopt -o dirnames 2>/dev/null
        COMPREPLY=()
        return
    fi

    # @repo / @_tag selectors on the plan commands.
    case $sub in
        install|upgrade|reinstall|remove|upgrade-all|download|install-new)
            if [[ $cur == @* ]]; then
                local names tags list body
                names=$(_slacker_repo_names "$config")
                tags=$(_slacker_build_tags "$config")
                list=""
                for body in $names; do list+=" @$body"; done
                for body in $tags;  do list+=" @$body"; done
                COMPREPLY=($(compgen -W "$list" -- "$cur"))
                return
            fi
            ;;
    esac

    # Positional arguments by subcommand.
    case $sub in
        remove|reinstall|info|revert-pkg|history)
            COMPREPLY=($(compgen -W "$(_slacker_installed_names)" -- "$cur")) ;;
        del-repo|pri-repo|vet-repo|trust-repo|distrust-repo|clean-cache|install-new|show-changelog)
            COMPREPLY=($(compgen -W "$(_slacker_repo_names "$config")" -- "$cur")) ;;
        install-template|remove-template|delete-template)
            COMPREPLY=($(compgen -W "$(_slacker_template_names "$config")" -- "$cur")) ;;
        pin)
            # First arg is repo:package; complete the repo: prefix.
            if [[ $cur != *:* ]]; then
                local r list=""
                for r in $(_slacker_repo_names "$config"); do list+=" $r:"; done
                compopt -o nospace 2>/dev/null
                COMPREPLY=($(compgen -W "$list" -- "$cur"))
            fi ;;
        unpin)
            COMPREPLY=($(compgen -W "$(_slacker_installed_names)" -- "$cur")) ;;
    esac
}

_slacker() {
    local cur prev words cword
    _init_completion 2>/dev/null || {
        # Minimal fallback when bash-completion's _init_completion is absent.
        COMPREPLY=()
        cur=${COMP_WORDS[COMP_CWORD]}
        prev=${COMP_WORDS[COMP_CWORD-1]}
    }

    # The subcommand is the first non-option word after `slacker`.
    local sub="" i
    for ((i = 1; i < COMP_CWORD; i++)); do
        case ${COMP_WORDS[i]} in
            --config-dir) ((i++)) ;;     # a flag that consumes the next word
            -*) ;;                       # any other flag
            *) sub=${COMP_WORDS[i]}; break ;;
        esac
    done

    # Position 1: complete the subcommand name.
    if [[ -z $sub ]]; then
        if [[ $cur == -* ]]; then
            COMPREPLY=($(compgen -W "$_slacker_global_flags" -- "$cur"))
        else
            COMPREPLY=($(compgen -W "$_slacker_commands" -- "$cur"))
        fi
        return
    fi

    _slacker_dispatch
}

complete -F _slacker slacker
