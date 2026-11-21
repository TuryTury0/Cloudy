# Makes the wake word test audio with the Windows voices (16 kHz, 16-bit, mono WAV) into target\wake-eval:
#   <phrase>\pos\<voice>_<n>.wav   the phrase said 30 ways (speed, pitch, loudness)
#   neg\<voice>_<n>.wav            every line of sentences.txt (normal talk, with words close to the phrases)
#   free\<voice>.wav               free.txt (the free-speech step of training)
# Run from anytask-next:  powershell -ExecutionPolicy Bypass -File crates\wake\eval\make_audio.ps1
# Then:                   cargo run --release -p anytask-wake --example eval
param([string]$Out = "target\wake-eval")

Add-Type -AssemblyName System.Speech
$here = Split-Path -Parent $MyInvocation.MyCommand.Path
$phrases = [ordered]@{ "heynova" = "Hey Nova"; "jarvis" = "Jarvis"; "anytask" = "Hey AnyTask" }
$voices = @{ "david" = "Microsoft David Desktop"; "zira" = "Microsoft Zira Desktop" }
$format = New-Object System.Speech.AudioFormat.SpeechAudioFormatInfo(16000, [System.Speech.AudioFormat.AudioBitsPerSample]::Sixteen, [System.Speech.AudioFormat.AudioChannel]::Mono)
# SAPI speed steps (about 12% each): -2 is a quarter slower, +2 a fifth faster, like a person saying it differently.
$rates = @(-2, -1, 0, 1, 2)
$pitches = @("low", "medium", "high")

function Say($voice, $rate, $ssmlBody, $path) {
    $synth = New-Object System.Speech.Synthesis.SpeechSynthesizer
    $synth.SelectVoice($voice)
    $synth.Rate = $rate
    $synth.SetOutputToWaveFile($path, $format)
    $ssml = "<speak version='1.0' xmlns='http://www.w3.org/2001/10/synthesis' xml:lang='en-US'>$ssmlBody</speak>"
    $synth.SpeakSsml($ssml)
    $synth.Dispose()
}

foreach ($dir in @("neg", "free")) { New-Item -ItemType Directory -Force -Path (Join-Path $Out $dir) | Out-Null }
foreach ($v in $voices.Keys) {
    $name = $voices[$v]
    foreach ($p in $phrases.Keys) {
        $dir = Join-Path $Out "$p\pos"
        New-Item -ItemType Directory -Force -Path $dir | Out-Null
        for ($n = 0; $n -lt 30; $n++) {
            $rate = $rates[$n % $rates.Count]
            $pitch = $pitches[[math]::Floor($n / $rates.Count) % $pitches.Count]
            $volume = 60 + (($n * 7) % 41)
            $body = "<prosody pitch='$pitch' volume='$volume'>$($phrases[$p])</prosody>"
            Say $name $rate $body (Join-Path $dir ("{0}_{1:d2}.wav" -f $v, $n))
        }
    }
    $i = 0
    foreach ($line in Get-Content (Join-Path $here "sentences.txt")) {
        if ($line.Trim() -eq "") { continue }
        $rate = $rates[($i % 3) + 1]
        $text = [System.Security.SecurityElement]::Escape($line)
        Say $name $rate $text (Join-Path $Out ("neg\{0}_{1:d3}.wav" -f $v, $i))
        $i++
    }
    $free = [System.Security.SecurityElement]::Escape((Get-Content (Join-Path $here "free.txt") -Raw))
    Say $name 0 $free (Join-Path $Out "free\$v.wav")
}
Write-Output "done: $Out"
