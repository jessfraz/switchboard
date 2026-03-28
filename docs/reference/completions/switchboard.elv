
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
            cand -h 'Print help'
            cand --help 'Print help'
            cand -V 'Print version'
            cand --version 'Print version'
            cand ns 'ns'
            cand tools 'tools'
            cand audit 'audit'
            cand op 'op'
        }
        &'switchboard;ns'= {
            cand --config 'config'
            cand -h 'Print help'
            cand --help 'Print help'
            cand list 'list'
        }
        &'switchboard;ns;list'= {
            cand --config 'config'
            cand --json 'json'
            cand -h 'Print help'
            cand --help 'Print help'
        }
        &'switchboard;tools'= {
            cand --config 'config'
            cand -h 'Print help'
            cand --help 'Print help'
            cand list 'list'
            cand describe 'describe'
        }
        &'switchboard;tools;list'= {
            cand --config 'config'
            cand --json 'json'
            cand -h 'Print help'
            cand --help 'Print help'
        }
        &'switchboard;tools;describe'= {
            cand --config 'config'
            cand --json 'json'
            cand -h 'Print help'
            cand --help 'Print help'
        }
        &'switchboard;audit'= {
            cand --config 'config'
            cand -h 'Print help'
            cand --help 'Print help'
            cand list 'list'
            cand show 'show'
        }
        &'switchboard;audit;list'= {
            cand --operation-id 'operation-id'
            cand --config 'config'
            cand --json 'json'
            cand -h 'Print help'
            cand --help 'Print help'
        }
        &'switchboard;audit;show'= {
            cand --config 'config'
            cand --json 'json'
            cand -h 'Print help'
            cand --help 'Print help'
        }
        &'switchboard;op'= {
            cand --config 'config'
            cand -h 'Print help'
            cand --help 'Print help'
            cand list 'list'
            cand show 'show'
            cand approve 'approve'
            cand reject 'reject'
            cand apply 'apply'
            cand undo 'undo'
        }
        &'switchboard;op;list'= {
            cand --config 'config'
            cand --pending 'pending'
            cand --json 'json'
            cand -h 'Print help'
            cand --help 'Print help'
        }
        &'switchboard;op;show'= {
            cand --config 'config'
            cand --json 'json'
            cand -h 'Print help'
            cand --help 'Print help'
        }
        &'switchboard;op;approve'= {
            cand --actor 'actor'
            cand --note 'note'
            cand --config 'config'
            cand --apply 'apply'
            cand --json 'json'
            cand -h 'Print help'
            cand --help 'Print help'
        }
        &'switchboard;op;reject'= {
            cand --actor 'actor'
            cand --note 'note'
            cand --config 'config'
            cand --json 'json'
            cand -h 'Print help'
            cand --help 'Print help'
        }
        &'switchboard;op;apply'= {
            cand --config 'config'
            cand --json 'json'
            cand -h 'Print help'
            cand --help 'Print help'
        }
        &'switchboard;op;undo'= {
            cand --config 'config'
            cand --apply 'apply'
            cand --json 'json'
            cand -h 'Print help'
            cand --help 'Print help'
        }
    ]
    $completions[$command]
}
