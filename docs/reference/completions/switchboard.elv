
use builtin;
use str;

set edit:completion:arg-completer[switchboard] = {|@words|
    fn spaces {|n|
        builtin:repeat $n ' ' | str:join ''
    }
    fn cand {|text desc|
        edit:complex-candidate $text &display=$text' '(spaces (- 14 (wcswidth $text)))$desc
    }
    var command = 'switchboard'
    for word $words[1..-1] {
        if (str:has-prefix $word '-') {
            break
        }
        set command = $command';'$word
    }
    var completions = [
        &'switchboard'= {
            cand --config 'config'
            cand --fields 'Select comma-separated paths within result fields; arrays apply each path to every row'
            cand --full 'Show full diagnostic output and pretty-printed JSON'
            cand -h 'Print help'
            cand --help 'Print help'
            cand -V 'Print version'
            cand --version 'Print version'
            cand read-batch 'Execute bounded read-only requests with durable, resumable page results'
            cand ns 'ns'
            cand doctor 'Inspect configuration, saved state, and CLI availability without authenticating'
            cand auth 'Verify provider authentication or preview adoption of an existing CLI session'
            cand tools 'tools'
            cand audit 'audit'
            cand op 'op'
        }
        &'switchboard;read-batch'= {
            cand --input 'JSON object containing an items array; use - to read stdin'
            cand --tool 'Curated read tool shared by each --args-json request'
            cand --ns 'Namespace shared by each --args-json request'
            cand --args-json 'One request''s arguments as an object; repeat for multiple queries or IDs'
            cand --checkpoint 'Durable results; successful pages are saved after each bounded wave'
            cand --resume 'Resume a checkpoint without the original input; bare --resume uses --checkpoint'
            cand --concurrency 'concurrency'
            cand --deadline-seconds 'deadline-seconds'
            cand --max-pages 'max-pages'
            cand --config 'config'
            cand --fields 'Select comma-separated paths within result fields; arrays apply each path to every row'
            cand --json 'json'
            cand --full 'Show full diagnostic output and pretty-printed JSON'
            cand -h 'Print help'
            cand --help 'Print help'
        }
        &'switchboard;ns'= {
            cand --config 'config'
            cand --fields 'Select comma-separated paths within result fields; arrays apply each path to every row'
            cand --full 'Show full diagnostic output and pretty-printed JSON'
            cand -h 'Print help'
            cand --help 'Print help'
            cand list 'list'
        }
        &'switchboard;ns;list'= {
            cand --config 'config'
            cand --fields 'Select comma-separated paths within result fields; arrays apply each path to every row'
            cand --json 'json'
            cand --full 'Show full diagnostic output and pretty-printed JSON'
            cand -h 'Print help'
            cand --help 'Print help'
        }
        &'switchboard;doctor'= {
            cand --ns 'Inspect one namespace instead of all configured namespaces'
            cand --config 'config'
            cand --fields 'Select comma-separated paths within result fields; arrays apply each path to every row'
            cand --json 'json'
            cand --full 'Show full diagnostic output and pretty-printed JSON'
            cand -h 'Print help'
            cand --help 'Print help'
        }
        &'switchboard;auth'= {
            cand --config 'config'
            cand --fields 'Select comma-separated paths within result fields; arrays apply each path to every row'
            cand --full 'Show full diagnostic output and pretty-printed JSON'
            cand -h 'Print help'
            cand --help 'Print help'
            cand check 'Perform a read-only provider identity request and verify the configured account'
            cand migration-preview 'Show an opt-in Google CLI migration without modifying configuration or credentials'
        }
        &'switchboard;auth;check'= {
            cand --ns 'ns'
            cand --run-id 'Share the one-recovery-attempt budget across commands in this task'
            cand --config 'config'
            cand --fields 'Select comma-separated paths within result fields; arrays apply each path to every row'
            cand --json 'json'
            cand --full 'Show full diagnostic output and pretty-printed JSON'
            cand -h 'Print help'
            cand --help 'Print help'
        }
        &'switchboard;auth;migration-preview'= {
            cand --ns 'ns'
            cand --config 'config'
            cand --fields 'Select comma-separated paths within result fields; arrays apply each path to every row'
            cand --verify 'Verify the saved CLI session through a live read before changing config'
            cand --json 'json'
            cand --full 'Show full diagnostic output and pretty-printed JSON'
            cand -h 'Print help'
            cand --help 'Print help'
        }
        &'switchboard;tools'= {
            cand --config 'config'
            cand --fields 'Select comma-separated paths within result fields; arrays apply each path to every row'
            cand --full 'Show full diagnostic output and pretty-printed JSON'
            cand -h 'Print help'
            cand --help 'Print help'
            cand list 'list'
            cand describe 'describe'
        }
        &'switchboard;tools;list'= {
            cand --provider 'Filter by provider identifier, for example google or github'
            cand --ns 'Filter by a configured namespace without resolving credentials'
            cand --search 'Search tool names and summaries (case insensitive)'
            cand --limit 'Maximum matching commands to show (default: 8; --full shows all)'
            cand --config 'config'
            cand --fields 'Select comma-separated paths within result fields; arrays apply each path to every row'
            cand --json 'json'
            cand --executable 'Include only tools with an implemented execution path'
            cand --full 'Show full diagnostic output and pretty-printed JSON'
            cand -h 'Print help'
            cand --help 'Print help'
        }
        &'switchboard;tools;describe'= {
            cand --ns 'Use this configured namespace in examples'
            cand --config 'config'
            cand --fields 'Select comma-separated paths within result fields; arrays apply each path to every row'
            cand --json 'json'
            cand --full 'Show full diagnostic output and pretty-printed JSON'
            cand -h 'Print help'
            cand --help 'Print help'
        }
        &'switchboard;audit'= {
            cand --config 'config'
            cand --fields 'Select comma-separated paths within result fields; arrays apply each path to every row'
            cand --full 'Show full diagnostic output and pretty-printed JSON'
            cand -h 'Print help'
            cand --help 'Print help'
            cand list 'list'
            cand show 'show'
        }
        &'switchboard;audit;list'= {
            cand --operation-id 'operation-id'
            cand --config 'config'
            cand --fields 'Select comma-separated paths within result fields; arrays apply each path to every row'
            cand --json 'json'
            cand --full 'Show full diagnostic output and pretty-printed JSON'
            cand -h 'Print help'
            cand --help 'Print help'
        }
        &'switchboard;audit;show'= {
            cand --config 'config'
            cand --fields 'Select comma-separated paths within result fields; arrays apply each path to every row'
            cand --json 'json'
            cand --full 'Show full diagnostic output and pretty-printed JSON'
            cand -h 'Print help'
            cand --help 'Print help'
        }
        &'switchboard;op'= {
            cand --config 'config'
            cand --fields 'Select comma-separated paths within result fields; arrays apply each path to every row'
            cand --full 'Show full diagnostic output and pretty-printed JSON'
            cand -h 'Print help'
            cand --help 'Print help'
            cand list 'list'
            cand show 'show'
            cand approve 'approve'
            cand reject 'reject'
            cand apply 'apply'
            cand verify 'Read back provider state without repeating the write'
            cand undo 'undo'
        }
        &'switchboard;op;list'= {
            cand --config 'config'
            cand --fields 'Select comma-separated paths within result fields; arrays apply each path to every row'
            cand --pending 'pending'
            cand --json 'json'
            cand --full 'Show full diagnostic output and pretty-printed JSON'
            cand -h 'Print help'
            cand --help 'Print help'
        }
        &'switchboard;op;show'= {
            cand --config 'config'
            cand --fields 'Select comma-separated paths within result fields; arrays apply each path to every row'
            cand --json 'json'
            cand --full 'Show full diagnostic output and pretty-printed JSON'
            cand -h 'Print help'
            cand --help 'Print help'
        }
        &'switchboard;op;approve'= {
            cand --actor 'actor'
            cand --note 'note'
            cand --config 'config'
            cand --fields 'Select comma-separated paths within result fields; arrays apply each path to every row'
            cand --apply 'apply'
            cand --json 'json'
            cand --full 'Show full diagnostic output and pretty-printed JSON'
            cand -h 'Print help'
            cand --help 'Print help'
        }
        &'switchboard;op;reject'= {
            cand --actor 'actor'
            cand --note 'note'
            cand --config 'config'
            cand --fields 'Select comma-separated paths within result fields; arrays apply each path to every row'
            cand --json 'json'
            cand --full 'Show full diagnostic output and pretty-printed JSON'
            cand -h 'Print help'
            cand --help 'Print help'
        }
        &'switchboard;op;apply'= {
            cand --config 'config'
            cand --fields 'Select comma-separated paths within result fields; arrays apply each path to every row'
            cand --json 'json'
            cand --full 'Show full diagnostic output and pretty-printed JSON'
            cand -h 'Print help'
            cand --help 'Print help'
        }
        &'switchboard;op;verify'= {
            cand --config 'config'
            cand --fields 'Select comma-separated paths within result fields; arrays apply each path to every row'
            cand --json 'json'
            cand --full 'Show full diagnostic output and pretty-printed JSON'
            cand -h 'Print help'
            cand --help 'Print help'
        }
        &'switchboard;op;undo'= {
            cand --config 'config'
            cand --fields 'Select comma-separated paths within result fields; arrays apply each path to every row'
            cand --apply 'apply'
            cand --json 'json'
            cand --full 'Show full diagnostic output and pretty-printed JSON'
            cand -h 'Print help'
            cand --help 'Print help'
        }
    ]
    $completions[$command]
}
