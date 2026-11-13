# Run this first; if needed, copy the browser offer when prompted, then press Enter.
# Requires a local automation test build with LITECORD_WEB_MANUAL=1 and an active share.
param([ValidateRange(1, 65535)][int]$Port = 39091)
. (Join-Path $PSScriptRoot 'lib.ps1')
while ($true) {
    $offer = $null
    try {
        $candidate = Get-Clipboard -Raw | ConvertFrom-Json -ErrorAction Stop
        if ($candidate.type -eq 'offer' -and $candidate.sdp -is [string] -and -not [string]::IsNullOrWhiteSpace($candidate.sdp)) {
            $offer = $candidate
        }
    } catch {
        # Copying this command replaces the offer. Wait for a fresh copy instead
        # of exposing a JSON parsing error or echoing private SDP to the terminal.
    }
    if ($null -ne $offer) { break }
    Write-Host 'The clipboard does not contain a valid WebRTC offer yet.'
    $choice = Read-Host 'In the browser, select ALL the text in the first field and copy it. Come back here and press Enter (or Q to quit)'
    if ($choice -match '^\s*q\s*$') { return }
}
$reply = Send-Litecord $Port (@{ cmd = 'web_share_answer'; offer = $offer } | ConvertTo-Json -Depth 8 -Compress)
if (-not $reply.ok) { throw $reply.error }
Set-Clipboard ($reply.answer | ConvertTo-Json -Depth 8 -Compress)
Write-Output 'Answer copied. Paste it into the second field of the page and click Connect.'
