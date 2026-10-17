# Runs after PowerShell profiles; the user's profile files are never modified.
$global:__GerminalOriginalPrompt = $function:prompt
function global:prompt {
    # Invoke the existing prompt first so its output and side effects are preserved.
    & $global:__GerminalOriginalPrompt
    if ($PWD.Provider.Name -eq 'FileSystem') {
        [Console]::Write("$([char]27)]9;9;$($PWD.ProviderPath)$([char]7)")
    }
}
