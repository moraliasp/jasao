#!/usr/bin/env bash
# Mede as metas da seção 3 de docs/plano.md num JSONL de 1 GB e imprime "meta × medido".
#   scripts/medir.sh            gera target/medir/1gb.jsonl (uma vez) e mede com o CLI de release
#   MEDIR_MB=256 MEDIR_N=9 scripts/medir.sh   outro tamanho / outro número de rodadas
# Tempo = mediana de N rodadas (`/usr/bin/time -l`, arquivo já no cache de páginas).
# Memória = maior "peak memory footprint" das rodadas: o que o processo aloca, sem as páginas
# limpas do mmap (é a meta "além do mmap"); o RSS máximo, que inclui o mmap, sai ao lado.
# Só macOS (`/usr/bin/time -l`). Não roda no CI: 1 GB e minutos de máquina.
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"

mb="${MEDIR_MB:-1024}"
n="${MEDIR_N:-5}"
dir=target/medir
arq="$dir/${mb}mb.jsonl"
mkdir -p "$dir"

cargo build --release -q -p cli
cargo build --release -q -p nucleo --example gerar
bin=target/release/jasao
[ -s "$arq" ] || { echo "gerando $arq…" >&2; target/release/examples/gerar "$arq" "$mb" 1 >&2; }
cat "$arq" >/dev/null # aquece o cache de páginas: mede o programa, não o disco

# mede <rótulo> <meta> <limite_s> <comando…>: N rodadas, tempo mediano e memória máxima
mede() {
  local rotulo="$1" meta="$2" limite="$3"
  shift 3
  local tempos=() pico=0 rss=0 log="$dir/time.log"
  for _ in $(seq "$n"); do
    /usr/bin/time -l "$@" >/dev/null 2>"$log"
    tempos+=("$(awk '/ real /{print $1}' "$log")")
    pico=$(awk -v m="$pico" '/peak memory footprint/{if ($1>m) m=$1} END{print m}' "$log")
    rss=$(awk -v m="$rss" '/maximum resident set size/{if ($1>m) m=$1} END{print m}' "$log")
  done
  local med
  med=$(printf '%s\n' "${tempos[@]}" | sort -n | awk '{a[NR]=$1} END{print a[int((NR+1)/2)]}')
  awk -v r="$rotulo" -v m="$meta" -v t="$med" -v l="$limite" -v p="$pico" -v s="$rss" 'BEGIN{
    printf "| %s | %s | %.2f s | %.0f MB | %.0f MB | %s |\n",
      r, m, t, p/1048576, s/1048576, (t < l ? "ok" : "NÃO")
  }'
}

echo
echo "$(sw_vers -productName) $(sw_vers -productVersion) · $(sysctl -n machdep.cpu.brand_string) · $(sysctl -n hw.ncpu) núcleos · $(($(sysctl -n hw.memsize) >> 30)) GB"
echo "jasão $(git describe --always --dirty) · $(stat -f %z "$arq") bytes · mediana de $n rodadas"
echo "carga antes de medir: $(sysctl -n vm.loadavg) (load average 1/5/15 min; alto = números pessimistas)"
echo
echo "| Medida (CLI, processo inteiro) | Meta | Medido | Pico alocado | RSS máximo | Meta |"
echo "|---|---|---|---|---|---|"
mede "primeira tela (abrir + 50 linhas)" "< 300 ms" 0.3 "$bin" janela "$arq" 0 50
mede "índice de linhas (abrir)" "< 2 s" 2 "$bin" linhas "$arq"
mede "estrutura completa (inferir)" "< 5 s" 5 "$bin" inferir "$arq"
mede "busca até o fim (literal ausente)" "< 3 s" 3 "$bin" buscar "$arq" "agulha inexistente"
mede "busca até o fim (regex ausente)" "< 3 s" 3 "$bin" buscar --regex "$arq" '"qtd":\d{3}'
mede "exportar Schema + validar tudo" "—" 1e9 "$bin" exportar --schema "$arq"
echo
echo "Meta de memória: pico alocado < 300 MB em todas as linhas."
