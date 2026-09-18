# Print an optspec for argparse to handle cmd's options that are independent of any subcommand.
function __fish_switchboard_global_optspecs
    string join \n config= h/help V/version
end

function __fish_switchboard_needs_command
    # Figure out if the current invocation already has a command.
    set -l cmd (commandline -opc)
    set -e cmd[1]
    argparse -s (__fish_switchboard_global_optspecs) -- $cmd 2>/dev/null
    or return
    if set -q argv[1]
        # Also print the command, so this can be used to figure out what it is.
        echo $argv[1]
        return 1
    end
    return 0
end

function __fish_switchboard_using_subcommand
    set -l cmd (__fish_switchboard_needs_command)
    test -z "$cmd"
    and return 1
    contains -- $cmd[1] $argv
end

complete -c switchboard -n "__fish_switchboard_needs_command" -l config -r -F
complete -c switchboard -n "__fish_switchboard_needs_command" -s h -l help -d 'Print help'
complete -c switchboard -n "__fish_switchboard_needs_command" -s V -l version -d 'Print version'
complete -c switchboard -n "__fish_switchboard_needs_command" -f -a "read-batch" -d 'Execute bounded read-only requests with durable, resumable page results'
complete -c switchboard -n "__fish_switchboard_needs_command" -f -a "ns"
complete -c switchboard -n "__fish_switchboard_needs_command" -f -a "doctor" -d 'Inspect configuration, saved state, and CLI availability without authenticating'
complete -c switchboard -n "__fish_switchboard_needs_command" -f -a "auth" -d 'Verify provider authentication or preview adoption of an existing CLI session'
complete -c switchboard -n "__fish_switchboard_needs_command" -f -a "tools"
complete -c switchboard -n "__fish_switchboard_needs_command" -f -a "audit"
complete -c switchboard -n "__fish_switchboard_needs_command" -f -a "op"
complete -c switchboard -n "__fish_switchboard_using_subcommand read-batch" -l input -d 'JSON object containing an items array of {id, tool, namespace, args}' -r -F
complete -c switchboard -n "__fish_switchboard_using_subcommand read-batch" -l checkpoint -d 'Durable results; successful pages are saved after each bounded wave' -r -F
complete -c switchboard -n "__fish_switchboard_using_subcommand read-batch" -l concurrency -r
complete -c switchboard -n "__fish_switchboard_using_subcommand read-batch" -l deadline-seconds -r
complete -c switchboard -n "__fish_switchboard_using_subcommand read-batch" -l max-pages -r
complete -c switchboard -n "__fish_switchboard_using_subcommand read-batch" -l config -r -F
complete -c switchboard -n "__fish_switchboard_using_subcommand read-batch" -l resume
complete -c switchboard -n "__fish_switchboard_using_subcommand read-batch" -l json
complete -c switchboard -n "__fish_switchboard_using_subcommand read-batch" -s h -l help -d 'Print help'
complete -c switchboard -n "__fish_switchboard_using_subcommand ns; and not __fish_seen_subcommand_from list" -l config -r -F
complete -c switchboard -n "__fish_switchboard_using_subcommand ns; and not __fish_seen_subcommand_from list" -s h -l help -d 'Print help'
complete -c switchboard -n "__fish_switchboard_using_subcommand ns; and not __fish_seen_subcommand_from list" -f -a "list"
complete -c switchboard -n "__fish_switchboard_using_subcommand ns; and __fish_seen_subcommand_from list" -l config -r -F
complete -c switchboard -n "__fish_switchboard_using_subcommand ns; and __fish_seen_subcommand_from list" -l json
complete -c switchboard -n "__fish_switchboard_using_subcommand ns; and __fish_seen_subcommand_from list" -s h -l help -d 'Print help'
complete -c switchboard -n "__fish_switchboard_using_subcommand doctor" -l ns -d 'Inspect one namespace instead of all configured namespaces' -r
complete -c switchboard -n "__fish_switchboard_using_subcommand doctor" -l config -r -F
complete -c switchboard -n "__fish_switchboard_using_subcommand doctor" -l json
complete -c switchboard -n "__fish_switchboard_using_subcommand doctor" -s h -l help -d 'Print help'
complete -c switchboard -n "__fish_switchboard_using_subcommand auth; and not __fish_seen_subcommand_from check migration-preview" -l config -r -F
complete -c switchboard -n "__fish_switchboard_using_subcommand auth; and not __fish_seen_subcommand_from check migration-preview" -s h -l help -d 'Print help'
complete -c switchboard -n "__fish_switchboard_using_subcommand auth; and not __fish_seen_subcommand_from check migration-preview" -f -a "check" -d 'Perform a read-only provider identity request and verify the configured account'
complete -c switchboard -n "__fish_switchboard_using_subcommand auth; and not __fish_seen_subcommand_from check migration-preview" -f -a "migration-preview" -d 'Show an opt-in Google CLI migration without modifying configuration or credentials'
complete -c switchboard -n "__fish_switchboard_using_subcommand auth; and __fish_seen_subcommand_from check" -l ns -r
complete -c switchboard -n "__fish_switchboard_using_subcommand auth; and __fish_seen_subcommand_from check" -l run-id -d 'Share the one-recovery-attempt budget across commands in this task' -r
complete -c switchboard -n "__fish_switchboard_using_subcommand auth; and __fish_seen_subcommand_from check" -l config -r -F
complete -c switchboard -n "__fish_switchboard_using_subcommand auth; and __fish_seen_subcommand_from check" -l json
complete -c switchboard -n "__fish_switchboard_using_subcommand auth; and __fish_seen_subcommand_from check" -s h -l help -d 'Print help'
complete -c switchboard -n "__fish_switchboard_using_subcommand auth; and __fish_seen_subcommand_from migration-preview" -l ns -r
complete -c switchboard -n "__fish_switchboard_using_subcommand auth; and __fish_seen_subcommand_from migration-preview" -l config -r -F
complete -c switchboard -n "__fish_switchboard_using_subcommand auth; and __fish_seen_subcommand_from migration-preview" -l verify -d 'Verify the saved CLI session through a live read before changing config'
complete -c switchboard -n "__fish_switchboard_using_subcommand auth; and __fish_seen_subcommand_from migration-preview" -l json
complete -c switchboard -n "__fish_switchboard_using_subcommand auth; and __fish_seen_subcommand_from migration-preview" -s h -l help -d 'Print help'
complete -c switchboard -n "__fish_switchboard_using_subcommand tools; and not __fish_seen_subcommand_from list describe" -l config -r -F
complete -c switchboard -n "__fish_switchboard_using_subcommand tools; and not __fish_seen_subcommand_from list describe" -s h -l help -d 'Print help'
complete -c switchboard -n "__fish_switchboard_using_subcommand tools; and not __fish_seen_subcommand_from list describe" -f -a "list"
complete -c switchboard -n "__fish_switchboard_using_subcommand tools; and not __fish_seen_subcommand_from list describe" -f -a "describe"
complete -c switchboard -n "__fish_switchboard_using_subcommand tools; and __fish_seen_subcommand_from list" -l provider -d 'Filter by provider identifier, for example google or github' -r
complete -c switchboard -n "__fish_switchboard_using_subcommand tools; and __fish_seen_subcommand_from list" -l ns -d 'Filter by a configured namespace without resolving credentials' -r
complete -c switchboard -n "__fish_switchboard_using_subcommand tools; and __fish_seen_subcommand_from list" -l search -d 'Search tool names and summaries (case insensitive)' -r
complete -c switchboard -n "__fish_switchboard_using_subcommand tools; and __fish_seen_subcommand_from list" -l config -r -F
complete -c switchboard -n "__fish_switchboard_using_subcommand tools; and __fish_seen_subcommand_from list" -l json
complete -c switchboard -n "__fish_switchboard_using_subcommand tools; and __fish_seen_subcommand_from list" -l executable -d 'Include only tools with an implemented execution path'
complete -c switchboard -n "__fish_switchboard_using_subcommand tools; and __fish_seen_subcommand_from list" -s h -l help -d 'Print help'
complete -c switchboard -n "__fish_switchboard_using_subcommand tools; and __fish_seen_subcommand_from describe" -l ns -d 'Use this configured namespace in examples' -r
complete -c switchboard -n "__fish_switchboard_using_subcommand tools; and __fish_seen_subcommand_from describe" -l config -r -F
complete -c switchboard -n "__fish_switchboard_using_subcommand tools; and __fish_seen_subcommand_from describe" -l json
complete -c switchboard -n "__fish_switchboard_using_subcommand tools; and __fish_seen_subcommand_from describe" -s h -l help -d 'Print help'
complete -c switchboard -n "__fish_switchboard_using_subcommand audit; and not __fish_seen_subcommand_from list show" -l config -r -F
complete -c switchboard -n "__fish_switchboard_using_subcommand audit; and not __fish_seen_subcommand_from list show" -s h -l help -d 'Print help'
complete -c switchboard -n "__fish_switchboard_using_subcommand audit; and not __fish_seen_subcommand_from list show" -f -a "list"
complete -c switchboard -n "__fish_switchboard_using_subcommand audit; and not __fish_seen_subcommand_from list show" -f -a "show"
complete -c switchboard -n "__fish_switchboard_using_subcommand audit; and __fish_seen_subcommand_from list" -l operation-id -r
complete -c switchboard -n "__fish_switchboard_using_subcommand audit; and __fish_seen_subcommand_from list" -l config -r -F
complete -c switchboard -n "__fish_switchboard_using_subcommand audit; and __fish_seen_subcommand_from list" -l json
complete -c switchboard -n "__fish_switchboard_using_subcommand audit; and __fish_seen_subcommand_from list" -s h -l help -d 'Print help'
complete -c switchboard -n "__fish_switchboard_using_subcommand audit; and __fish_seen_subcommand_from show" -l config -r -F
complete -c switchboard -n "__fish_switchboard_using_subcommand audit; and __fish_seen_subcommand_from show" -l json
complete -c switchboard -n "__fish_switchboard_using_subcommand audit; and __fish_seen_subcommand_from show" -s h -l help -d 'Print help'
complete -c switchboard -n "__fish_switchboard_using_subcommand op; and not __fish_seen_subcommand_from list show approve reject apply verify undo" -l config -r -F
complete -c switchboard -n "__fish_switchboard_using_subcommand op; and not __fish_seen_subcommand_from list show approve reject apply verify undo" -s h -l help -d 'Print help'
complete -c switchboard -n "__fish_switchboard_using_subcommand op; and not __fish_seen_subcommand_from list show approve reject apply verify undo" -f -a "list"
complete -c switchboard -n "__fish_switchboard_using_subcommand op; and not __fish_seen_subcommand_from list show approve reject apply verify undo" -f -a "show"
complete -c switchboard -n "__fish_switchboard_using_subcommand op; and not __fish_seen_subcommand_from list show approve reject apply verify undo" -f -a "approve"
complete -c switchboard -n "__fish_switchboard_using_subcommand op; and not __fish_seen_subcommand_from list show approve reject apply verify undo" -f -a "reject"
complete -c switchboard -n "__fish_switchboard_using_subcommand op; and not __fish_seen_subcommand_from list show approve reject apply verify undo" -f -a "apply"
complete -c switchboard -n "__fish_switchboard_using_subcommand op; and not __fish_seen_subcommand_from list show approve reject apply verify undo" -f -a "verify" -d 'Read back provider state without repeating the write'
complete -c switchboard -n "__fish_switchboard_using_subcommand op; and not __fish_seen_subcommand_from list show approve reject apply verify undo" -f -a "undo"
complete -c switchboard -n "__fish_switchboard_using_subcommand op; and __fish_seen_subcommand_from list" -l config -r -F
complete -c switchboard -n "__fish_switchboard_using_subcommand op; and __fish_seen_subcommand_from list" -l pending
complete -c switchboard -n "__fish_switchboard_using_subcommand op; and __fish_seen_subcommand_from list" -l json
complete -c switchboard -n "__fish_switchboard_using_subcommand op; and __fish_seen_subcommand_from list" -s h -l help -d 'Print help'
complete -c switchboard -n "__fish_switchboard_using_subcommand op; and __fish_seen_subcommand_from show" -l config -r -F
complete -c switchboard -n "__fish_switchboard_using_subcommand op; and __fish_seen_subcommand_from show" -l json
complete -c switchboard -n "__fish_switchboard_using_subcommand op; and __fish_seen_subcommand_from show" -s h -l help -d 'Print help'
complete -c switchboard -n "__fish_switchboard_using_subcommand op; and __fish_seen_subcommand_from approve" -l actor -r
complete -c switchboard -n "__fish_switchboard_using_subcommand op; and __fish_seen_subcommand_from approve" -l note -r
complete -c switchboard -n "__fish_switchboard_using_subcommand op; and __fish_seen_subcommand_from approve" -l config -r -F
complete -c switchboard -n "__fish_switchboard_using_subcommand op; and __fish_seen_subcommand_from approve" -l apply
complete -c switchboard -n "__fish_switchboard_using_subcommand op; and __fish_seen_subcommand_from approve" -l json
complete -c switchboard -n "__fish_switchboard_using_subcommand op; and __fish_seen_subcommand_from approve" -s h -l help -d 'Print help'
complete -c switchboard -n "__fish_switchboard_using_subcommand op; and __fish_seen_subcommand_from reject" -l actor -r
complete -c switchboard -n "__fish_switchboard_using_subcommand op; and __fish_seen_subcommand_from reject" -l note -r
complete -c switchboard -n "__fish_switchboard_using_subcommand op; and __fish_seen_subcommand_from reject" -l config -r -F
complete -c switchboard -n "__fish_switchboard_using_subcommand op; and __fish_seen_subcommand_from reject" -l json
complete -c switchboard -n "__fish_switchboard_using_subcommand op; and __fish_seen_subcommand_from reject" -s h -l help -d 'Print help'
complete -c switchboard -n "__fish_switchboard_using_subcommand op; and __fish_seen_subcommand_from apply" -l config -r -F
complete -c switchboard -n "__fish_switchboard_using_subcommand op; and __fish_seen_subcommand_from apply" -l json
complete -c switchboard -n "__fish_switchboard_using_subcommand op; and __fish_seen_subcommand_from apply" -s h -l help -d 'Print help'
complete -c switchboard -n "__fish_switchboard_using_subcommand op; and __fish_seen_subcommand_from verify" -l config -r -F
complete -c switchboard -n "__fish_switchboard_using_subcommand op; and __fish_seen_subcommand_from verify" -l json
complete -c switchboard -n "__fish_switchboard_using_subcommand op; and __fish_seen_subcommand_from verify" -s h -l help -d 'Print help'
complete -c switchboard -n "__fish_switchboard_using_subcommand op; and __fish_seen_subcommand_from undo" -l config -r -F
complete -c switchboard -n "__fish_switchboard_using_subcommand op; and __fish_seen_subcommand_from undo" -l apply
complete -c switchboard -n "__fish_switchboard_using_subcommand op; and __fish_seen_subcommand_from undo" -l json
complete -c switchboard -n "__fish_switchboard_using_subcommand op; and __fish_seen_subcommand_from undo" -s h -l help -d 'Print help'
