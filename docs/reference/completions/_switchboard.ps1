
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
            [CompletionResult]::new('-h', '-h', [CompletionResultType]::ParameterName, 'Print help')
            [CompletionResult]::new('--help', '--help', [CompletionResultType]::ParameterName, 'Print help')
            [CompletionResult]::new('-V', '-V ', [CompletionResultType]::ParameterName, 'Print version')
            [CompletionResult]::new('--version', '--version', [CompletionResultType]::ParameterName, 'Print version')
            [CompletionResult]::new('ns', 'ns', [CompletionResultType]::ParameterValue, 'ns')
            [CompletionResult]::new('tools', 'tools', [CompletionResultType]::ParameterValue, 'tools')
            [CompletionResult]::new('audit', 'audit', [CompletionResultType]::ParameterValue, 'audit')
            [CompletionResult]::new('op', 'op', [CompletionResultType]::ParameterValue, 'op')
            break
        }
        'switchboard;ns' {
            [CompletionResult]::new('--config', '--config', [CompletionResultType]::ParameterName, 'config')
            [CompletionResult]::new('-h', '-h', [CompletionResultType]::ParameterName, 'Print help')
            [CompletionResult]::new('--help', '--help', [CompletionResultType]::ParameterName, 'Print help')
            [CompletionResult]::new('list', 'list', [CompletionResultType]::ParameterValue, 'list')
            break
        }
        'switchboard;ns;list' {
            [CompletionResult]::new('--config', '--config', [CompletionResultType]::ParameterName, 'config')
            [CompletionResult]::new('--json', '--json', [CompletionResultType]::ParameterName, 'json')
            [CompletionResult]::new('-h', '-h', [CompletionResultType]::ParameterName, 'Print help')
            [CompletionResult]::new('--help', '--help', [CompletionResultType]::ParameterName, 'Print help')
            break
        }
        'switchboard;tools' {
            [CompletionResult]::new('--config', '--config', [CompletionResultType]::ParameterName, 'config')
            [CompletionResult]::new('-h', '-h', [CompletionResultType]::ParameterName, 'Print help')
            [CompletionResult]::new('--help', '--help', [CompletionResultType]::ParameterName, 'Print help')
            [CompletionResult]::new('list', 'list', [CompletionResultType]::ParameterValue, 'list')
            [CompletionResult]::new('describe', 'describe', [CompletionResultType]::ParameterValue, 'describe')
            break
        }
        'switchboard;tools;list' {
            [CompletionResult]::new('--config', '--config', [CompletionResultType]::ParameterName, 'config')
            [CompletionResult]::new('--json', '--json', [CompletionResultType]::ParameterName, 'json')
            [CompletionResult]::new('-h', '-h', [CompletionResultType]::ParameterName, 'Print help')
            [CompletionResult]::new('--help', '--help', [CompletionResultType]::ParameterName, 'Print help')
            break
        }
        'switchboard;tools;describe' {
            [CompletionResult]::new('--config', '--config', [CompletionResultType]::ParameterName, 'config')
            [CompletionResult]::new('--json', '--json', [CompletionResultType]::ParameterName, 'json')
            [CompletionResult]::new('-h', '-h', [CompletionResultType]::ParameterName, 'Print help')
            [CompletionResult]::new('--help', '--help', [CompletionResultType]::ParameterName, 'Print help')
            break
        }
        'switchboard;audit' {
            [CompletionResult]::new('--config', '--config', [CompletionResultType]::ParameterName, 'config')
            [CompletionResult]::new('-h', '-h', [CompletionResultType]::ParameterName, 'Print help')
            [CompletionResult]::new('--help', '--help', [CompletionResultType]::ParameterName, 'Print help')
            [CompletionResult]::new('list', 'list', [CompletionResultType]::ParameterValue, 'list')
            [CompletionResult]::new('show', 'show', [CompletionResultType]::ParameterValue, 'show')
            break
        }
        'switchboard;audit;list' {
            [CompletionResult]::new('--operation-id', '--operation-id', [CompletionResultType]::ParameterName, 'operation-id')
            [CompletionResult]::new('--config', '--config', [CompletionResultType]::ParameterName, 'config')
            [CompletionResult]::new('--json', '--json', [CompletionResultType]::ParameterName, 'json')
            [CompletionResult]::new('-h', '-h', [CompletionResultType]::ParameterName, 'Print help')
            [CompletionResult]::new('--help', '--help', [CompletionResultType]::ParameterName, 'Print help')
            break
        }
        'switchboard;audit;show' {
            [CompletionResult]::new('--config', '--config', [CompletionResultType]::ParameterName, 'config')
            [CompletionResult]::new('--json', '--json', [CompletionResultType]::ParameterName, 'json')
            [CompletionResult]::new('-h', '-h', [CompletionResultType]::ParameterName, 'Print help')
            [CompletionResult]::new('--help', '--help', [CompletionResultType]::ParameterName, 'Print help')
            break
        }
        'switchboard;op' {
            [CompletionResult]::new('--config', '--config', [CompletionResultType]::ParameterName, 'config')
            [CompletionResult]::new('-h', '-h', [CompletionResultType]::ParameterName, 'Print help')
            [CompletionResult]::new('--help', '--help', [CompletionResultType]::ParameterName, 'Print help')
            [CompletionResult]::new('list', 'list', [CompletionResultType]::ParameterValue, 'list')
            [CompletionResult]::new('show', 'show', [CompletionResultType]::ParameterValue, 'show')
            [CompletionResult]::new('approve', 'approve', [CompletionResultType]::ParameterValue, 'approve')
            [CompletionResult]::new('reject', 'reject', [CompletionResultType]::ParameterValue, 'reject')
            [CompletionResult]::new('apply', 'apply', [CompletionResultType]::ParameterValue, 'apply')
            [CompletionResult]::new('undo', 'undo', [CompletionResultType]::ParameterValue, 'undo')
            break
        }
        'switchboard;op;list' {
            [CompletionResult]::new('--config', '--config', [CompletionResultType]::ParameterName, 'config')
            [CompletionResult]::new('--pending', '--pending', [CompletionResultType]::ParameterName, 'pending')
            [CompletionResult]::new('--json', '--json', [CompletionResultType]::ParameterName, 'json')
            [CompletionResult]::new('-h', '-h', [CompletionResultType]::ParameterName, 'Print help')
            [CompletionResult]::new('--help', '--help', [CompletionResultType]::ParameterName, 'Print help')
            break
        }
        'switchboard;op;show' {
            [CompletionResult]::new('--config', '--config', [CompletionResultType]::ParameterName, 'config')
            [CompletionResult]::new('--json', '--json', [CompletionResultType]::ParameterName, 'json')
            [CompletionResult]::new('-h', '-h', [CompletionResultType]::ParameterName, 'Print help')
            [CompletionResult]::new('--help', '--help', [CompletionResultType]::ParameterName, 'Print help')
            break
        }
        'switchboard;op;approve' {
            [CompletionResult]::new('--actor', '--actor', [CompletionResultType]::ParameterName, 'actor')
            [CompletionResult]::new('--note', '--note', [CompletionResultType]::ParameterName, 'note')
            [CompletionResult]::new('--config', '--config', [CompletionResultType]::ParameterName, 'config')
            [CompletionResult]::new('--apply', '--apply', [CompletionResultType]::ParameterName, 'apply')
            [CompletionResult]::new('--json', '--json', [CompletionResultType]::ParameterName, 'json')
            [CompletionResult]::new('-h', '-h', [CompletionResultType]::ParameterName, 'Print help')
            [CompletionResult]::new('--help', '--help', [CompletionResultType]::ParameterName, 'Print help')
            break
        }
        'switchboard;op;reject' {
            [CompletionResult]::new('--actor', '--actor', [CompletionResultType]::ParameterName, 'actor')
            [CompletionResult]::new('--note', '--note', [CompletionResultType]::ParameterName, 'note')
            [CompletionResult]::new('--config', '--config', [CompletionResultType]::ParameterName, 'config')
            [CompletionResult]::new('--json', '--json', [CompletionResultType]::ParameterName, 'json')
            [CompletionResult]::new('-h', '-h', [CompletionResultType]::ParameterName, 'Print help')
            [CompletionResult]::new('--help', '--help', [CompletionResultType]::ParameterName, 'Print help')
            break
        }
        'switchboard;op;apply' {
            [CompletionResult]::new('--config', '--config', [CompletionResultType]::ParameterName, 'config')
            [CompletionResult]::new('--json', '--json', [CompletionResultType]::ParameterName, 'json')
            [CompletionResult]::new('-h', '-h', [CompletionResultType]::ParameterName, 'Print help')
            [CompletionResult]::new('--help', '--help', [CompletionResultType]::ParameterName, 'Print help')
            break
        }
        'switchboard;op;undo' {
            [CompletionResult]::new('--config', '--config', [CompletionResultType]::ParameterName, 'config')
            [CompletionResult]::new('--apply', '--apply', [CompletionResultType]::ParameterName, 'apply')
            [CompletionResult]::new('--json', '--json', [CompletionResultType]::ParameterName, 'json')
            [CompletionResult]::new('-h', '-h', [CompletionResultType]::ParameterName, 'Print help')
            [CompletionResult]::new('--help', '--help', [CompletionResultType]::ParameterName, 'Print help')
            break
        }
    })

    $completions.Where{ $_.CompletionText -like "$wordToComplete*" } |
        Sort-Object -Property ListItemText
}
