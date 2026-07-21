<#
.SYNOPSIS
  AgamaLang compiler/runner for .aga files.

.PARAMETER Command
  run     - compile + run (default)
  build   - compile only
  clean   - remove all .exe from project root
  list    - show available examples/
  test    - compile & run all examples/*.aga

.PARAMETER File
  Path to .aga file.

.PARAMETER Out
  Output .exe name (optional).

.EXAMPLE
  .\aga hello.aga          (ищет в текущей папке и examples/)
  .\aga run hello.aga
  .\aga build exit.aga
  .\aga build exit.aga out.exe
  .\aga clean
  .\aga list
  .\aga test
#>

param(
  [Parameter(Position = 0)]
  [string]$Command = "run",

  [Parameter(Position = 1)]
  [string]$File = "",

  [Parameter(Position = 2)]
  [string]$Out = ""
)

$ProjectRoot = Split-Path -Parent $PSCommandPath
$Compiler    = Join-Path $ProjectRoot "target\debug\agamalang.exe"
$CargoExe    = Join-Path $env:USERPROFILE ".cargo\bin\cargo.exe"

function Step($Label, $Color = "Green") {
  Write-Host -NoNewline "[" -ForegroundColor DarkGray
  Write-Host -NoNewline (Get-Date -Format "HH:mm:ss") -ForegroundColor DarkGray
  Write-Host -NoNewline "] " -ForegroundColor DarkGray
  Write-Host $Label -ForegroundColor $Color
}

# ——————— Ensure compiler is built ———————
function Ensure-Compiler {
  if (-not (Test-Path -LiteralPath $Compiler)) {
    Step "Building compiler..." Yellow
    $null = & $CargoExe build 2>&1
    if ($LASTEXITCODE -ne 0) {
      Step "FAIL: compiler build error" Red
      exit 1
    }
    Step "Compiler built" Green
  }
}

# ——————— Build .aga -> .exe ———————
function Build-Aga {
  param([string]$SrcFile, [string]$OutName)

  # Auto-search in examples/ if file not found directly
  if (-not (Test-Path -LiteralPath $SrcFile -PathType Leaf)) {
    $inExamples = Join-Path (Join-Path $ProjectRoot "examples") $SrcFile
    if (Test-Path -LiteralPath $inExamples -PathType Leaf) {
      $SrcFile = $inExamples
    } else {
      Step "FAIL: file not found: $SrcFile" Red
      exit 1
    }
  }
    Step "FAIL: file not found: $SrcFile" Red
    exit 1
  }

  Step "Compiling: $SrcFile" Cyan
  Push-Location -LiteralPath $ProjectRoot
  $output = & $CargoExe run -- $SrcFile $OutName 2>&1
  Pop-Location
  $lastLine = $output | Select-Object -Last 1

  if ($LASTEXITCODE -ne 0) {
    Write-Host "`n$output" -ForegroundColor Red
    Step "FAIL: compilation error" Red
    exit 1
  }

  # Extract exe name from last line: "... -> 'file.exe'"
  $exeRel = ""
  if ($lastLine -match "'(?<exe>[^']+\.exe)'") {
    $exeRel = $Matches['exe']
  } else {
    $exeRel = [System.IO.Path]::GetFileNameWithoutExtension($SrcFile) + ".exe"
  }

  $exePath = Join-Path $ProjectRoot $exeRel
  Step "Built: $exeRel" Green
  return $exePath
}

# ——————— Run .exe ———————
function Run-Exe {
  param([string]$ExePath)

  if (-not (Test-Path -LiteralPath $ExePath)) {
    Step "FAIL: exe not found: $ExePath" Red
    exit 1
  }

  Step "Running: $ExePath" Magenta
  & (Resolve-Path -LiteralPath $ExePath)
  $ec = $LASTEXITCODE
  $color = if ($ec -eq 0) { "Green" } else { "Yellow" }
  Step "Exit: $ec" $color
  return $ec
}

# ——————— Commands ———————
function Do-Run {
  if (-not $File) {
    Step "Usage: aga run <file.aga>" Red
    exit 1
  }
  Ensure-Compiler
  $exe = Build-Aga -SrcFile $File -OutName $Out
  Run-Exe -ExePath $exe
}

function Do-Build {
  if (-not $File) {
    Step "Usage: aga build <file.aga>" Red
    exit 1
  }
  Ensure-Compiler
  $null = Build-Aga -SrcFile $File -OutName $Out
}

function Do-Clean {
  $removed = 0
  Get-ChildItem -Path $ProjectRoot -Filter "*.exe" | ForEach-Object {
    if ($_.Directory.Name -ne "pedump" -and $_.Directory.Name -ne "target") {
      Remove-Item -LiteralPath $_.FullName -Force
      $removed++
    }
  }
  if ($removed -eq 0) {
    Step "No exe files to clean" DarkGray
  } else {
    Step "Removed $removed exe files" Green
  }
}

function Do-List {
  $exampleDir = Join-Path $ProjectRoot "examples"
  $examples = Get-ChildItem -Path $exampleDir -Filter "*.aga"
  if ($examples.Count -eq 0) {
    Step "No .aga files in examples/" Yellow
    return
  }

  Step "Examples in examples/:"
  $examples | Sort-Object Name | ForEach-Object {
    Write-Host "  $($_.Name)" -ForegroundColor Cyan
  }
}

function Do-Test {
  Ensure-Compiler
  $exampleDir = Join-Path $ProjectRoot "examples"
  $examples = Get-ChildItem -Path $exampleDir -Filter "*.aga" | Sort-Object Name
  $passed = 0
  $failed = 0

  Step "Running $($examples.Count) tests..." Cyan

  foreach ($ex in $examples) {
    Write-Host -NoNewline "  $($ex.Name) ... " -ForegroundColor Cyan

    # Compile
    Push-Location -LiteralPath $ProjectRoot
    $output = & $CargoExe run -- $ex.FullName 2>&1
    Pop-Location
    if ($LASTEXITCODE -ne 0) {
      Write-Host "COMPILE FAIL" -ForegroundColor Red
      $failed++
      continue
    }

    # Find output exe
    $exeName = [System.IO.Path]::GetFileNameWithoutExtension($ex.Name) + ".exe"
    $exePath = Join-Path $ProjectRoot $exeName
    if (-not (Test-Path -LiteralPath $exePath)) {
      Write-Host "NO EXE" -ForegroundColor Red
      $failed++
      continue
    }

    # Run
    $null = & $exePath 2>&1
    $ec = $LASTEXITCODE
    if ($ec -eq 0) {
      Write-Host "OK" -ForegroundColor Green
      $passed++
    } else {
      Write-Host "FAIL (exit $ec)" -ForegroundColor Red
      $failed++
    }
  }

  Step "Passed: $passed | Failed: $failed" $(if ($failed -eq 0) { "Green" } else { "Red" })
  exit $failed
}

# ——————— Dispatch ———————
switch ($Command.ToLower()) {
  "run"   { Do-Run }
  "build" { Do-Build }
  "clean" { Do-Clean }
  "list"  { Do-List }
  "test"  { Do-Test }
  default {
    if ($Command -like "*.aga") {
      $global:File = $Command
      $global:Command = "run"
      Do-Run
    } else {
      Step "Unknown command: $Command" Red
      Write-Host "  Commands: run, build, clean, list, test" -ForegroundColor DarkGray
      exit 1
    }
  }
}
