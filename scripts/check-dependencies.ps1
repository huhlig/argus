<#
 Copyright 2026 Hans W. Uhlig

 Licensed under the Apache License, Version 2.0 (the "License");
 you may not use this file except in compliance with the License.
 You may obtain a copy of the License at

     http://www.apache.org/licenses/LICENSE-2.0

 Unless required by applicable law or agreed to in writing, software
 distributed under the License is distributed on an "AS IS" BASIS,
 WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
 See the License for the specific language governing permissions and
 limitations under the License.
#>

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
