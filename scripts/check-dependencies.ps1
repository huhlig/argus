$ErrorActionPreference = 'Stop'

$metadata = cargo metadata --format-version 1 --no-deps | ConvertFrom-Json
$core = $metadata.packages | Where-Object name -eq 'argus-core'
$testSupport = $metadata.packages | Where-Object name -eq 'argus-test-support'

$workspaceNames = @($metadata.packages.name)
$coreWorkspaceDependencies = @($core.dependencies | Where-Object {
    $workspaceNames -contains $_.name
})

if ($coreWorkspaceDependencies.Count -ne 0) {
    throw "argus-core depends on workspace crates: $($coreWorkspaceDependencies.name -join ', ')"
}

$violations = $metadata.packages | Where-Object {
    $_.name -ne 'argus-test-support' -and
    $_.dependencies.name -contains $testSupport.name
}

if ($violations) {
    throw "production crates depend on argus-test-support: $($violations.name -join ', ')"
}

Write-Output 'Workspace dependency direction is valid.'
