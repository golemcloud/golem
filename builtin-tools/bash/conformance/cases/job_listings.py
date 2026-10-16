"""What `jobs` lists in command and process substitutions and in pipeline stages: the jobs' states
when they started, as bash lists them there, so a throttle that counts running jobs ends.

Each case name starts with `job listings: `.
"""

CASES = [
    (
        "job listings: a finished job is not running in a substitution or a stage",
        "sleep 0.2 & sleep 1; echo \"r=[$(jobs -r)]\"; jobs -r | wc -l; n=$(jobs -rp | wc -l); echo n=$n",
    ),
    (
        "job listings: a throttle on jobs -r",
        "for i in $(seq 10); do\n  while [ \"$(jobs -r | wc -l)\" -ge 4 ]; do sleep 0.1; done\n  sleep 0.3 &\ndone\nwait; echo done",
    ),
    (
        "job listings: a throttle on jobs -p",
        "for i in $(seq 10); do\n  while [ \"$(jobs -p | wc -l)\" -ge 4 ]; do sleep 0.1; done\n  sleep 0.3 &\ndone\nwait; echo done",
    ),
    # A substitution leaves out the jobs that had ended, but the last one started (`$!`).
    (
        "job listings: a substitution lists the running jobs and the last one started",
        "sleep 0.1 & sleep 0.1 & sleep 1\n"
        "echo \"c=[$(jobs)]\"; echo \"p=[$(jobs -p | wc -l)]\"; x=$(jobs | cat); echo \"x=[$x]\"\n"
        "f() { jobs; }; x=$(f); echo \"f=[$x]\"; cat <(jobs); jobs | cat; echo top; jobs; wait\n"
        "sleep 3 & sleep 0.1 & sleep 3 & sleep 0.1 & sleep 1\n"
        "echo \"c=[$(jobs)]\"; echo \"p=[$(jobs -p | wc -l)] r=[$(jobs -r | wc -l)]\"; kill %1 %3; wait",
    ),
    # Only a stage that is `jobs` itself lists them; a function or compound command does not.
    (
        "job listings: only a jobs stage lists the jobs of its shell",
        "sleep 0.1 & sleep 0.1 & sleep 1\n"
        "f() { jobs; }\n"
        "echo fpipe:; f | cat; echo while:; echo | while read; do jobs; done; echo if:; if true; then jobs; fi | cat\n"
        "echo group:; { jobs; } | cat; echo sub:; ( jobs ) | cat; echo x=$(echo | { jobs; })\n"
        "echo first:; jobs | cat; echo last:; echo | jobs; wait",
    ),
]
