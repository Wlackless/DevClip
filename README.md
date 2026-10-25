# DevClip

Gravador de tela em Rust para Windows, com uma cápsula discreta no topo da tela.
Passe o mouse sobre ela para abrir os controles. Ao afastar o mouse, ela recolhe.

- **Gravar tela** inicia a captura do monitor principal, incluindo o cursor.
- **Pausar / Continuar** suspende e retoma o vídeo, sem incluir o tempo pausado.
- **Finalizar** salva o MP4 em **Vídeos\DevClip**. O ícone de pasta abre esse diretório.
- O **×** finaliza a gravação em andamento antes de sair.
- A interface fica fora da gravação por meio de `WDA_EXCLUDEFROMCAPTURE` do Windows.

| Qualidade | Resolução máxima | Quadros por segundo |
| --- | --- | --- |
| Leve | 720p | 30 |
| Alta | 1080p | 30 |
| Original | Resolução do monitor | 60 |

A proporção da tela é preservada. Telas menores não são ampliadas. A qualidade
é escolhida antes de iniciar uma gravação. Esta versão grava **vídeo sem áudio**.

## Executar

Requer **Windows 10 versão 2004 ou superior**, Rust estável com toolchain MSVC
e **FFmpeg com libx264** disponível no PATH. Também é possível colocar
`ffmpeg.exe` ao lado de `devclip.exe`. O FFmpeg já está instalado neste computador.

```powershell
cargo run --release
```

Ou compile com `cargo build --release` e abra `target\release\devclip.exe`.
Nenhuma janela de terminal é aberta na versão release.

Quando a cápsula está com foco, **Espaço** grava/pausa/continua e **Esc** finaliza.
Os controles permanecem sempre à frente das outras janelas.

## Desenvolvimento e verificação

```powershell
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
```

O teste de integração abaixo captura a tela local por alguns segundos, verifica
que a pausa não produz quadros e usa FFprobe/FFmpeg para validar o MP4. Requer
sessão gráfica desbloqueada, FFmpeg e FFprobe no PATH. Apaga seu próprio vídeo
quando termina com sucesso.

```powershell
cargo test captures_mp4_and_omits_paused_time -- --ignored --nocapture
```

## Estrutura

- `src/app.rs`: interface egui/eframe, expansão e estados dos controles.
- `src/platform.rs`: integração Win32, posição e exclusão da janela da captura.
- `src/recorder.rs`: Windows Graphics Capture, pausa, cadência e codificação MP4.

A captura usa Windows Graphics Capture. Um relógio separado entrega quadros ao
FFmpeg em frequência fixa, repetindo o último quadro quando a tela está parada.
Durante a pausa, nenhum quadro é enviado. O codificador recebe EOF ao finalizar,
escreve o índice do MP4 e só então a interface confirma que o vídeo foi salvo.

Erros são exibidos na cápsula. Em falhas de codificação, o diagnóstico `.log`
fica junto ao vídeo; arquivos de gravações interrompidas podem estar incompletos.
Mudanças de resolução e interrupções da captura exigem iniciar outro vídeo.
Original em telas de alta resolução pode exigir bastante CPU e espaço em disco.

A exclusão da janela usa a
[API oficial SetWindowDisplayAffinity](https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-setwindowdisplayaffinity).
Se o Windows rejeitar essa configuração, o aplicativo impede o início da gravação
e exibe o erro. A exclusão requer a composição de área de trabalho do Windows.
