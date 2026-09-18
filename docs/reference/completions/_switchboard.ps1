
using namespace System.Management.Automation
using namespace System.Management.Automation.Language

Register-ArgumentCompleter -Native -CommandName 'switchboard' -ScriptBlock {
    param($wordToComplete, $commandAst, $cursorPosition)

    $commandElements = $commandAst.CommandElements
    $command = @(
        'switchboard'
        for ($i = 1; $i -lt $commandElements.Count; $i++) {
            $element = $commandElements[$i]
            if ($element -isnot [StringConstantExpressionAst] -or
                $element.StringConstantType -ne [StringConstantType]::BareWord -or
                $element.Value.StartsWith('-') -or
                $element.Value -eq $wordToComplete) {
                break
        }
        $element.Value
    }) -join ';'

    $completions = @(switch ($command) {
        'switchboard' {
            [CompletionResult]::new('--config', '--config', [CompletionResultType]::ParameterName, 'config')
            [CompletionResult]::new('--fields', '--fields', [CompletionResultType]::ParameterName, 'Select comma-separated paths within result fields; arrays apply each path to every row')
            [CompletionResult]::new('--full', '--full', [CompletionResultType]::ParameterName, 'Show full diagnostic output and pretty-printed JSON')
            [CompletionResult]::new('-h', '-h', [CompletionResultType]::ParameterName, 'Print help')
            [CompletionResult]::new('--help', '--help', [CompletionResultType]::ParameterName, 'Print help')
            [CompletionResult]::new('-V', '-V ', [CompletionResultType]::ParameterName, 'Print version')
            [CompletionResult]::new('--version', '--version', [CompletionResultType]::ParameterName, 'Print version')
            [CompletionResult]::new('read-batch', 'read-batch', [CompletionResultType]::ParameterValue, 'Execute bounded read-only requests with durable, resumable page results')
            [CompletionResult]::new('ns', 'ns', [CompletionResultType]::ParameterValue, 'ns')
            [CompletionResult]::new('doctor', 'doctor', [CompletionResultType]::ParameterValue, 'Inspect configuration, saved state, and CLI availability without authenticating')
            [CompletionResult]::new('auth', 'auth', [CompletionResultType]::ParameterValue, 'Verify provider authentication or preview adoption of an existing CLI session')
            [CompletionResult]::new('tools', 'tools', [CompletionResultType]::ParameterValue, 'tools')
            [CompletionResult]::new('audit', 'audit', [CompletionResultType]::ParameterValue, 'audit')
            [CompletionResult]::new('op', 'op', [CompletionResultType]::ParameterValue, 'op')
            break
        }
        'switchboard;read-batch' {
            [CompletionResult]::new('--input', '--input', [CompletionResultType]::ParameterName, 'JSON object containing an items array; use - to read stdin')
            [CompletionResult]::new('--tool', '--tool', [CompletionResultType]::ParameterName, 'Curated read tool shared by each --args-json request')
            [CompletionResult]::new('--ns', '--ns', [CompletionResultType]::ParameterName, 'Namespace shared by each --args-json request')
            [CompletionResult]::new('--args-json', '--args-json', [CompletionResultType]::ParameterName, 'One request''s arguments as an object; repeat for multiple queries or IDs')
            [CompletionResult]::new('--checkpoint', '--checkpoint', [CompletionResultType]::ParameterName, 'Durable results; successful pages are saved after each bounded wave')
            [CompletionResult]::new('--resume', '--resume', [CompletionResultType]::ParameterName, 'Resume a checkpoint without the original input; bare --resume uses --checkpoint')
            [CompletionResult]::new('--concurrency', '--concurrency', [CompletionResultType]::ParameterName, 'concurrency')
            [CompletionResult]::new('--deadline-seconds', '--deadline-seconds', [CompletionResultType]::ParameterName, 'deadline-seconds')
            [CompletionResult]::new('--max-pages', '--max-pages', [CompletionResultType]::ParameterName, 'max-pages')
            [CompletionResult]::new('--config', '--config', [CompletionResultType]::ParameterName, 'config')
            [CompletionResult]::new('--fields', '--fields', [CompletionResultType]::ParameterName, 'Select comma-separated paths within result fields; arrays apply each path to every row')
            [CompletionResult]::new('--json', '--json', [CompletionResultType]::ParameterName, 'json')
            [CompletionResult]::new('--full', '--full', [CompletionResultType]::ParameterName, 'Show full diagnostic output and pretty-printed JSON')
            [CompletionResult]::new('-h', '-h', [CompletionResultType]::ParameterName, 'Print help')
            [CompletionResult]::new('--help', '--help', [CompletionResultType]::ParameterName, 'Print help')
            break
        }
        'switchboard;ns' {
            [CompletionResult]::new('--config', '--config', [CompletionResultType]::ParameterName, 'config')
            [CompletionResult]::new('--fields', '--fields', [CompletionResultType]::ParameterName, 'Select comma-separated paths within result fields; arrays apply each path to every row')
            [CompletionResult]::new('--full', '--full', [CompletionResultType]::ParameterName, 'Show full diagnostic output and pretty-printed JSON')
            [CompletionResult]::new('-h', '-h', [CompletionResultType]::ParameterName, 'Print help')
            [CompletionResult]::new('--help', '--help', [CompletionResultType]::ParameterName, 'Print help')
            [CompletionResult]::new('list', 'list', [CompletionResultType]::ParameterValue, 'list')
            break
        }
        'switchboard;ns;list' {
            [CompletionResult]::new('--config', '--config', [CompletionResultType]::ParameterName, 'config')
            [CompletionResult]::new('--fields', '--fields', [CompletionResultType]::ParameterName, 'Select comma-separated paths within result fields; arrays apply each path to every row')
            [CompletionResult]::new('--json', '--json', [CompletionResultType]::ParameterName, 'json')
            [CompletionResult]::new('--full', '--full', [CompletionResultType]::ParameterName, 'Show full diagnostic output and pretty-printed JSON')
            [CompletionResult]::new('-h', '-h', [CompletionResultType]::ParameterName, 'Print help')
            [CompletionResult]::new('--help', '--help', [CompletionResultType]::ParameterName, 'Print help')
            break
        }
        'switchboard;doctor' {
            [CompletionResult]::new('--ns', '--ns', [CompletionResultType]::ParameterName, 'Inspect one namespace instead of all configured namespaces')
            [CompletionResult]::new('--config', '--config', [CompletionResultType]::ParameterName, 'config')
            [CompletionResult]::new('--fields', '--fields', [CompletionResultType]::ParameterName, 'Select comma-separated paths within result fields; arrays apply each path to every row')
            [CompletionResult]::new('--json', '--json', [CompletionResultType]::ParameterName, 'json')
            [CompletionResult]::new('--full', '--full', [CompletionResultType]::ParameterName, 'Show full diagnostic output and pretty-printed JSON')
            [CompletionResult]::new('-h', '-h', [CompletionResultType]::ParameterName, 'Print help')
            [CompletionResult]::new('--help', '--help', [CompletionResultType]::ParameterName, 'Print help')
            break
        }
        'switchboard;auth' {
            [CompletionResult]::new('--config', '--config', [CompletionResultType]::ParameterName, 'config')
            [CompletionResult]::new('--fields', '--fields', [CompletionResultType]::ParameterName, 'Select comma-separated paths within result fields; arrays apply each path to every row')
            [CompletionResult]::new('--full', '--full', [CompletionResultType]::ParameterName, 'Show full diagnostic output and pretty-printed JSON')
            [CompletionResult]::new('-h', '-h', [CompletionResultType]::ParameterName, 'Print help')
            [CompletionResult]::new('--help', '--help', [CompletionResultType]::ParameterName, 'Print help')
            [CompletionResult]::new('check', 'check', [CompletionResultType]::ParameterValue, 'Perform a read-only provider identity request and verify the configured account')
            [CompletionResult]::new('migration-preview', 'migration-preview', [CompletionResultType]::ParameterValue, 'Show an opt-in Google CLI migration without modifying configuration or credentials')
            break
        }
        'switchboard;auth;check' {
            [CompletionResult]::new('--ns', '--ns', [CompletionResultType]::ParameterName, 'ns')
            [CompletionResult]::new('--run-id', '--run-id', [CompletionResultType]::ParameterName, 'Share the one-recovery-attempt budget across commands in this task')
            [CompletionResult]::new('--config', '--config', [CompletionResultType]::ParameterName, 'config')
            [CompletionResult]::new('--fields', '--fields', [CompletionResultType]::ParameterName, 'Select comma-separated paths within result fields; arrays apply each path to every row')
            [CompletionResult]::new('--json', '--json', [CompletionResultType]::ParameterName, 'json')
            [CompletionResult]::new('--full', '--full', [CompletionResultType]::ParameterName, 'Show full diagnostic output and pretty-printed JSON')
            [CompletionResult]::new('-h', '-h', [CompletionResultType]::ParameterName, 'Print help')
            [CompletionResult]::new('--help', '--help', [CompletionResultType]::ParameterName, 'Print help')
            break
        }
        'switchboard;auth;migration-preview' {
            [CompletionResult]::new('--ns', '--ns', [CompletionResultType]::ParameterName, 'ns')
            [CompletionResult]::new('--config', '--config', [CompletionResultType]::ParameterName, 'config')
            [CompletionResult]::new('--fields', '--fields', [CompletionResultType]::ParameterName, 'Select comma-separated paths within result fields; arrays apply each path to every row')
            [CompletionResult]::new('--verify', '--verify', [CompletionResultType]::ParameterName, 'Verify the saved CLI session through a live read before changing config')
            [CompletionResult]::new('--json', '--json', [CompletionResultType]::ParameterName, 'json')
            [CompletionResult]::new('--full', '--full', [CompletionResultType]::ParameterName, 'Show full diagnostic output and pretty-printed JSON')
            [CompletionResult]::new('-h', '-h', [CompletionResultType]::ParameterName, 'Print help')
            [CompletionResult]::new('--help', '--help', [CompletionResultType]::ParameterName, 'Print help')
            break
        }
        'switchboard;tools' {
            [CompletionResult]::new('--config', '--config', [CompletionResultType]::ParameterName, 'config')
            [CompletionResult]::new('--fields', '--fields', [CompletionResultType]::ParameterName, 'Select comma-separated paths within result fields; arrays apply each path to every row')
            [CompletionResult]::new('--full', '--full', [CompletionResultType]::ParameterName, 'Show full diagnostic output and pretty-printed JSON')
            [CompletionResult]::new('-h', '-h', [CompletionResultType]::ParameterName, 'Print help')
            [CompletionResult]::new('--help', '--help', [CompletionResultType]::ParameterName, 'Print help')
            [CompletionResult]::new('list', 'list', [CompletionResultType]::ParameterValue, 'list')
            [CompletionResult]::new('describe', 'describe', [CompletionResultType]::ParameterValue, 'describe')
            break
        }
        'switchboard;tools;list' {
            [CompletionResult]::new('--provider', '--provider', [CompletionResultType]::ParameterName, 'Filter by provider identifier, for example google or github')
            [CompletionResult]::new('--ns', '--ns', [CompletionResultType]::ParameterName, 'Filter by a configured namespace without resolving credentials')
            [CompletionResult]::new('--search', '--search', [CompletionResultType]::ParameterName, 'Search tool names and summaries (case insensitive)')
            [CompletionResult]::new('--limit', '--limit', [CompletionResultType]::ParameterName, 'Maximum matching commands to show (default: 8; --full shows all)')
            [CompletionResult]::new('--config', '--config', [CompletionResultType]::ParameterName, 'config')
            [CompletionResult]::new('--fields', '--fields', [CompletionResultType]::ParameterName, 'Select comma-separated paths within result fields; arrays apply each path to every row')
            [CompletionResult]::new('--json', '--json', [CompletionResultType]::ParameterName, 'json')
            [CompletionResult]::new('--executable', '--executable', [CompletionResultType]::ParameterName, 'Include only tools with an implemented execution path')
            [CompletionResult]::new('--full', '--full', [CompletionResultType]::ParameterName, 'Show full diagnostic output and pretty-printed JSON')
            [CompletionResult]::new('-h', '-h', [CompletionResultType]::ParameterName, 'Print help')
            [CompletionResult]::new('--help', '--help', [CompletionResultType]::ParameterName, 'Print help')
            break
        }
        'switchboard;tools;describe' {
            [CompletionResult]::new('--ns', '--ns', [CompletionResultType]::ParameterName, 'Use this configured namespace in examples')
            [CompletionResult]::new('--config', '--config', [CompletionResultType]::ParameterName, 'config')
            [CompletionResult]::new('--fields', '--fields', [CompletionResultType]::ParameterName, 'Select comma-separated paths within result fields; arrays apply each path to every row')
            [CompletionResult]::new('--json', '--json', [CompletionResultType]::ParameterName, 'json')
            [CompletionResult]::new('--full', '--full', [CompletionResultType]::ParameterName, 'Show full diagnostic output and pretty-printed JSON')
            [CompletionResult]::new('-h', '-h', [CompletionResultType]::ParameterName, 'Print help')
            [CompletionResult]::new('--help', '--help', [CompletionResultType]::ParameterName, 'Print help')
            break
        }
        'switchboard;audit' {
            [CompletionResult]::new('--config', '--config', [CompletionResultType]::ParameterName, 'config')
            [CompletionResult]::new('--fields', '--fields', [CompletionResultType]::ParameterName, 'Select comma-separated paths within result fields; arrays apply each path to every row')
            [CompletionResult]::new('--full', '--full', [CompletionResultType]::ParameterName, 'Show full diagnostic output and pretty-printed JSON')
            [CompletionResult]::new('-h', '-h', [CompletionResultType]::ParameterName, 'Print help')
            [CompletionResult]::new('--help', '--help', [CompletionResultType]::ParameterName, 'Print help')
            [CompletionResult]::new('list', 'list', [CompletionResultType]::ParameterValue, 'list')
            [CompletionResult]::new('show', 'show', [CompletionResultType]::ParameterValue, 'show')
            break
        }
        'switchboard;audit;list' {
            [CompletionResult]::new('--operation-id', '--operation-id', [CompletionResultType]::ParameterName, 'operation-id')
            [CompletionResult]::new('--config', '--config', [CompletionResultType]::ParameterName, 'config')
            [CompletionResult]::new('--fields', '--fields', [CompletionResultType]::ParameterName, 'Select comma-separated paths within result fields; arrays apply each path to every row')
            [CompletionResult]::new('--json', '--json', [CompletionResultType]::ParameterName, 'json')
            [CompletionResult]::new('--full', '--full', [CompletionResultType]::ParameterName, 'Show full diagnostic output and pretty-printed JSON')
            [CompletionResult]::new('-h', '-h', [CompletionResultType]::ParameterName, 'Print help')
            [CompletionResult]::new('--help', '--help', [CompletionResultType]::ParameterName, 'Print help')
            break
        }
        'switchboard;audit;show' {
            [CompletionResult]::new('--config', '--config', [CompletionResultType]::ParameterName, 'config')
            [CompletionResult]::new('--fields', '--fields', [CompletionResultType]::ParameterName, 'Select comma-separated paths within result fields; arrays apply each path to every row')
            [CompletionResult]::new('--json', '--json', [CompletionResultType]::ParameterName, 'json')
            [CompletionResult]::new('--full', '--full', [CompletionResultType]::ParameterName, 'Show full diagnostic output and pretty-printed JSON')
            [CompletionResult]::new('-h', '-h', [CompletionResultType]::ParameterName, 'Print help')
            [CompletionResult]::new('--help', '--help', [CompletionResultType]::ParameterName, 'Print help')
            break
        }
        'switchboard;op' {
            [CompletionResult]::new('--config', '--config', [CompletionResultType]::ParameterName, 'config')
            [CompletionResult]::new('--fields', '--fields', [CompletionResultType]::ParameterName, 'Select comma-separated paths within result fields; arrays apply each path to every row')
            [CompletionResult]::new('--full', '--full', [CompletionResultType]::ParameterName, 'Show full diagnostic output and pretty-printed JSON')
            [CompletionResult]::new('-h', '-h', [CompletionResultType]::ParameterName, 'Print help')
            [CompletionResult]::new('--help', '--help', [CompletionResultType]::ParameterName, 'Print help')
            [CompletionResult]::new('list', 'list', [CompletionResultType]::ParameterValue, 'list')
            [CompletionResult]::new('show', 'show', [CompletionResultType]::ParameterValue, 'show')
            [CompletionResult]::new('approve', 'approve', [CompletionResultType]::ParameterValue, 'approve')
            [CompletionResult]::new('reject', 'reject', [CompletionResultType]::ParameterValue, 'reject')
            [CompletionResult]::new('apply', 'apply', [CompletionResultType]::ParameterValue, 'apply')
            [CompletionResult]::new('verify', 'verify', [CompletionResultType]::ParameterValue, 'Read back provider state without repeating the write')
            [CompletionResult]::new('undo', 'undo', [CompletionResultType]::ParameterValue, 'undo')
            break
        }
        'switchboard;op;list' {
            [CompletionResult]::new('--config', '--config', [CompletionResultType]::ParameterName, 'config')
            [CompletionResult]::new('--fields', '--fields', [CompletionResultType]::ParameterName, 'Select comma-separated paths within result fields; arrays apply each path to every row')
            [CompletionResult]::new('--pending', '--pending', [CompletionResultType]::ParameterName, 'pending')
            [CompletionResult]::new('--json', '--json', [CompletionResultType]::ParameterName, 'json')
            [CompletionResult]::new('--full', '--full', [CompletionResultType]::ParameterName, 'Show full diagnostic output and pretty-printed JSON')
            [CompletionResult]::new('-h', '-h', [CompletionResultType]::ParameterName, 'Print help')
            [CompletionResult]::new('--help', '--help', [CompletionResultType]::ParameterName, 'Print help')
            break
        }
        'switchboard;op;show' {
            [CompletionResult]::new('--config', '--config', [CompletionResultType]::ParameterName, 'config')
            [CompletionResult]::new('--fields', '--fields', [CompletionResultType]::ParameterName, 'Select comma-separated paths within result fields; arrays apply each path to every row')
            [CompletionResult]::new('--json', '--json', [CompletionResultType]::ParameterName, 'json')
            [CompletionResult]::new('--full', '--full', [CompletionResultType]::ParameterName, 'Show full diagnostic output and pretty-printed JSON')
            [CompletionResult]::new('-h', '-h', [CompletionResultType]::ParameterName, 'Print help')
            [CompletionResult]::new('--help', '--help', [CompletionResultType]::ParameterName, 'Print help')
            break
        }
        'switchboard;op;approve' {
            [CompletionResult]::new('--actor', '--actor', [CompletionResultType]::ParameterName, 'actor')
            [CompletionResult]::new('--note', '--note', [CompletionResultType]::ParameterName, 'note')
            [CompletionResult]::new('--config', '--config', [CompletionResultType]::ParameterName, 'config')
            [CompletionResult]::new('--fields', '--fields', [CompletionResultType]::ParameterName, 'Select comma-separated paths within result fields; arrays apply each path to every row')
            [CompletionResult]::new('--apply', '--apply', [CompletionResultType]::ParameterName, 'apply')
            [CompletionResult]::new('--json', '--json', [CompletionResultType]::ParameterName, 'json')
            [CompletionResult]::new('--full', '--full', [CompletionResultType]::ParameterName, 'Show full diagnostic output and pretty-printed JSON')
            [CompletionResult]::new('-h', '-h', [CompletionResultType]::ParameterName, 'Print help')
            [CompletionResult]::new('--help', '--help', [CompletionResultType]::ParameterName, 'Print help')
            break
        }
        'switchboard;op;reject' {
            [CompletionResult]::new('--actor', '--actor', [CompletionResultType]::ParameterName, 'actor')
            [CompletionResult]::new('--note', '--note', [CompletionResultType]::ParameterName, 'note')
            [CompletionResult]::new('--config', '--config', [CompletionResultType]::ParameterName, 'config')
            [CompletionResult]::new('--fields', '--fields', [CompletionResultType]::ParameterName, 'Select comma-separated paths within result fields; arrays apply each path to every row')
            [CompletionResult]::new('--json', '--json', [CompletionResultType]::ParameterName, 'json')
            [CompletionResult]::new('--full', '--full', [CompletionResultType]::ParameterName, 'Show full diagnostic output and pretty-printed JSON')
            [CompletionResult]::new('-h', '-h', [CompletionResultType]::ParameterName, 'Print help')
            [CompletionResult]::new('--help', '--help', [CompletionResultType]::ParameterName, 'Print help')
            break
        }
        'switchboard;op;apply' {
            [CompletionResult]::new('--config', '--config', [CompletionResultType]::ParameterName, 'config')
            [CompletionResult]::new('--fields', '--fields', [CompletionResultType]::ParameterName, 'Select comma-separated paths within result fields; arrays apply each path to every row')
            [CompletionResult]::new('--json', '--json', [CompletionResultType]::ParameterName, 'json')
            [CompletionResult]::new('--full', '--full', [CompletionResultType]::ParameterName, 'Show full diagnostic output and pretty-printed JSON')
            [CompletionResult]::new('-h', '-h', [CompletionResultType]::ParameterName, 'Print help')
            [CompletionResult]::new('--help', '--help', [CompletionResultType]::ParameterName, 'Print help')
            break
        }
        'switchboard;op;verify' {
            [CompletionResult]::new('--config', '--config', [CompletionResultType]::ParameterName, 'config')
            [CompletionResult]::new('--fields', '--fields', [CompletionResultType]::ParameterName, 'Select comma-separated paths within result fields; arrays apply each path to every row')
            [CompletionResult]::new('--json', '--json', [CompletionResultType]::ParameterName, 'json')
            [CompletionResult]::new('--full', '--full', [CompletionResultType]::ParameterName, 'Show full diagnostic output and pretty-printed JSON')
            [CompletionResult]::new('-h', '-h', [CompletionResultType]::ParameterName, 'Print help')
            [CompletionResult]::new('--help', '--help', [CompletionResultType]::ParameterName, 'Print help')
            break
        }
        'switchboard;op;undo' {
            [CompletionResult]::new('--config', '--config', [CompletionResultType]::ParameterName, 'config')
            [CompletionResult]::new('--fields', '--fields', [CompletionResultType]::ParameterName, 'Select comma-separated paths within result fields; arrays apply each path to every row')
            [CompletionResult]::new('--apply', '--apply', [CompletionResultType]::ParameterName, 'apply')
            [CompletionResult]::new('--json', '--json', [CompletionResultType]::ParameterName, 'json')
            [CompletionResult]::new('--full', '--full', [CompletionResultType]::ParameterName, 'Show full diagnostic output and pretty-printed JSON')
            [CompletionResult]::new('-h', '-h', [CompletionResultType]::ParameterName, 'Print help')
            [CompletionResult]::new('--help', '--help', [CompletionResultType]::ParameterName, 'Print help')
            break
        }
    })

    $completions.Where{ $_.CompletionText -like "$wordToComplete*" } |
        Sort-Object -Property ListItemText
}
