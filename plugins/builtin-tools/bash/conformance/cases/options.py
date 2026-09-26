"""Shell options: set -e/-u/-x/-f/-n/-a, pipefail and POSIX mode."""

CASES = [
    ("option: errexit stops the script", 'set -e; echo before; false; echo not-reached'),
    ("option: errexit ignores conditions", 'set -e; if false; then :; fi; false || true; ! true; false && true; echo survived'),
    ("option: errexit in functions and subshells", 'set -e; f() { false; echo in-f; }; f || echo f-failed; (false; echo in-sub); echo not-reached'),
    ("option: errexit and command substitution", 'set -e; x=$(false; echo sub); echo "[$x]"; y=$(false); echo not-reached'),
    ("option: errexit in pipelines", 'set -e; false | true; echo last-wins; set -o pipefail; false | true; echo not-reached', ["option.pipefail"]),
    ("option: errexit with local", 'set -e; f() { local v=$(false); echo local-hides; }; f'),
    ("option: nounset errors", 'set -u; echo ${unset_var}; echo not-reached'),
    ("option: nounset and positionals", 'set -u; f() { echo "${1-none}" "$#"; echo "$@"; }; f; echo ok'),
    ("option: nounset with arrays", 'set -u; a=(); echo "${#a[@]}"; echo "${a[@]}" done'),
    ("option: xtrace", 'set -x; x=1; echo "$x" done; f() { :; }; f arg; set +x; echo quiet', ["option.xtrace"]),
    ("option: xtrace with PS4", 'PS4="> "; set -x; echo traced; set +x', ["option.xtrace"]),
    ("option: noglob", 'cd /tmp; touch g1; set -f; echo g*; set +f; echo g*', ["option.noglob"]),
    ("option: noexec", 'set -n; echo not-run; exit 3', ["option.noexec"]),
    ("option: pipefail status", 'set -o pipefail; (exit 2) | (exit 3) | true; echo $?; true | (exit 4); echo $?'),
    ("option: set -o and +o", 'set -o errexit; case $- in *e*) echo on;; esac; set +o errexit; case $- in *e*) echo still;; *) echo off;; esac'),
    ("option: posix mode", 'set -o posix; echo ${POSIXLY_CORRECT-unset}; case $SHELLOPTS in *posix*) echo posix;; esac', ["option.posix"]),
    ("option: inherited by children", 'set -e; bash -c \'case $- in *e*) echo child-e;; *) echo child-plain;; esac\''),
    ("option: shopt inherit_errexit", 'set -e; shopt -s inherit_errexit; x=$(false; echo leaked); echo "[$x]"'),
]
