#!/usr/bin/env bash
# Mede as metas da seção 3 de docs/plano.md num JSONL de 1 GB (e a 1ª tela do mesmo conteúdo como
# JSON formatado) e imprime "meta × medido".
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
if [ -s "$arq" ]; then
  echo "reaproveitando $arq já existente (apague-o para gerar de novo)" >&2
else
  echo "gerando $arq…" >&2
  target/release/examples/gerar "$arq" "$mb" 1 >&2
fi
# O mesmo conteúdo como um documento JSON formatado (array, um campo por linha): a máscara lê
# documento em ordem, por blocos, e a 1ª tela tem de ler só o 1º. Medido com e sem máscara: a
# diferença é o custo da máscara. Sem meta: com ~50 M linhas o índice de linhas (8 bytes por
# linha, montado inteiro ao abrir) sozinho passa dos 300 ms e dos 300 MB (docs/decisoes.md).
fmt="$dir/${mb}mb.formatado.json"
if [ ! -s "$fmt" ]; then
  echo "gerando $fmt…" >&2
  awk 'BEGIN{print "["} {gsub(/,"/, ",\n    \""); printf "%s  %s", (NR > 1 ? ",\n" : ""), $0} END{print "\n]"}' \
    "$arq" >"$fmt"
fi
cat "$fmt" >/dev/null
cat "$arq" >/dev/null # aquece o cache de páginas: mede o programa, não o disco

memoria_mb=300 # meta "além do mmap" do plano, conferida em cada linha
falhou=0
# mede <rótulo> <meta> <limite_s|-> <comando…>: N rodadas, tempo mediano e memória máxima.
# Limite "-" = medida sem meta de tempo (só a de memória vale); "=" = só informativa (nenhuma vale).
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
  awk -v r="$rotulo" -v m="$meta" -v t="$med" -v l="$limite" -v p="$pico" -v s="$rss" \
    -v mm="$memoria_mb" 'BEGIN{
    tempo = (l == "-" || l == "=") ? "—" : (t < l ? "ok" : "NÃO")
    mem = (l == "=") ? "—" : (p / 1048576 < mm) ? "ok" : "NÃO"
    printf "| %s | %s | %.2f s | %s | %.0f MB | %s | %.0f MB |\n",
      r, m, t, tempo, p/1048576, mem, s/1048576
    exit (tempo == "NÃO" || mem == "NÃO")
  }' || falhou=1
}

echo
echo "$(sw_vers -productName) $(sw_vers -productVersion) · $(sysctl -n machdep.cpu.brand_string) · $(sysctl -n hw.ncpu) núcleos · $(($(sysctl -n hw.memsize) >> 30)) GB"
echo "jasão $(git describe --always --dirty) · $(stat -f %z "$arq") bytes · mediana de $n rodadas"
echo "carga antes de medir: $(sysctl -n vm.loadavg) (load average 1/5/15 min; alto = números pessimistas)"
echo
echo "| Medida (CLI, processo inteiro) | Meta de tempo | Medido | Tempo | Pico alocado | < $memoria_mb MB | RSS máximo (com mmap) |"
echo "|---|---|---|---|---|---|---|"
mede "primeira tela (abrir + 50 linhas)" "< 300 ms" 0.3 "$bin" janela "$arq" 0 50
mede "primeira tela, JSON formatado" "sem meta" = "$bin" janela "$fmt" 0 50
mede "idem, --sem-mascara (base)" "sem meta" = "$bin" janela "$fmt" 0 50 --sem-mascara
mede "índice de linhas (abrir)" "< 2 s" 2 "$bin" linhas "$arq"
mede "estrutura completa (inferir)" "< 5 s" 5 "$bin" inferir "$arq"
mede "busca até o fim (literal ausente)" "< 3 s" 3 "$bin" buscar "$arq" "agulha inexistente"
mede "busca até o fim (regex ausente)" "< 3 s" 3 "$bin" buscar --regex "$arq" '"qtd":\d{3}'
mede "exportar Schema + validar tudo" "sem meta" - "$bin" exportar --schema "$arq"
echo
if [ "$falhou" = 0 ]; then echo "Todas as metas batidas."; else echo "Há meta NÃO batida (ver tabela)."; fi
exit "$falhou"
