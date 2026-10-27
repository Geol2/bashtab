# Fast startup for Git Bash Tabs: replaces /etc/profile without spawning
# any external process (process creation is very slow on this machine).

export MSYSTEM=MINGW64
export MSYSTEM_PREFIX=/mingw64
export MSYSTEM_CHOST=x86_64-w64-mingw32
export MINGW_PREFIX=/mingw64
export PATH="/mingw64/bin:/usr/local/bin:/usr/bin:/bin:$PATH"
export MANPATH="/mingw64/share/man:/usr/local/man:/usr/share/man:/usr/man:/share/man"
export LANG="${LANG:-ko_KR.UTF-8}"
export SHELL=/usr/bin/bash
export HISTFILE="$HOME/.bash_history"
export TERM=xterm-256color

alias ls='ls -F --color=auto --show-control-chars'
alias ll='ls -l'
alias la='ls -la'
alias grep='grep --color=auto'

# git branch for the prompt, read from .git/HEAD using builtins only
__gbt_branch() {
  __GBT_BRANCH=
  local d=$PWD git= head=
  while :; do
    if [[ -f $d/.git/HEAD ]]; then
      git=$d/.git
    elif [[ -f $d/.git ]]; then
      read -r git < "$d/.git"
      git=${git#gitdir: }
      [[ $git != /* && $git != ?:* ]] && git=$d/$git
    fi
    [[ -n $git ]] && break
    [[ -z $d || $d == / ]] && return
    d=${d%/*}
  done
  [[ -f $git/HEAD ]] || return
  read -r head < "$git/HEAD"
  if [[ $head == ref:\ refs/heads/* ]]; then
    __GBT_BRANCH=" (${head#ref: refs/heads/})"
  else
    __GBT_BRANCH=" (${head:0:7}...)"
  fi
}
# folders cd'ed into, appended for bashtab's "recent folders" (it trims the log)
__GBT_LAST=$PWD
__gbt_recent() {
  [[ $PWD == "$__GBT_LAST" ]] && return
  __GBT_LAST=$PWD
  [[ -n $BASHTAB_RECENT ]] && printf '%s\n' "$PWD" >> "$BASHTAB_RECENT"
}
PROMPT_COMMAND="__gbt_branch; __gbt_recent${PROMPT_COMMAND:+; $PROMPT_COMMAND}"
PS1='\[\e]0;\w\a\]\n\[\e[32m\]\u@\h \[\e[35m\]$MSYSTEM \[\e[33m\]\w\[\e[36m\]${__GBT_BRANCH}\[\e[0m\]\n$ '

# git tab-completion, loaded on the first Tab (sourcing it costs ~1s here)
__gbt_git_completion() {
  unset -f __gbt_git_completion
  . /mingw64/share/git/completion/git-completion.bash
  __git_wrap__git_main "$@"
}
[[ -f /mingw64/share/git/completion/git-completion.bash ]] &&
  complete -o bashdefault -o default -o nospace -F __gbt_git_completion git

# user's own config
[[ -f ~/.bashrc ]] && . ~/.bashrc
