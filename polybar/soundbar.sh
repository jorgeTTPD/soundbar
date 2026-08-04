#!/usr/bin/env bash

# Definir la ruta
FIFO="/tmp/soundbar.fifo"
PIDFILE="/tmp/soundbar.pid"

# Asegurar que el FIFO exista y esté limpio
if [ ! -p "$FIFO" ]; then
    mkfifo "$FIFO"
fi

# Comprobar si soundbar ya está corriendo (via PID file + cmdline)
soundbar_running() {
    local pid
    [ -f "$PIDFILE" ] || return 1
    pid="$(cat "$PIDFILE" 2>/dev/null)"
    [ -n "$pid" ] || return 1
    [ -d "/proc/$pid" ] || return 1
    tr '\0' ' ' < "/proc/$pid/cmdline" 2>/dev/null | grep -q "soundbar" || return 1
    return 0
}

# Iniciar soundbar en segundo plano si no está corriendo
if ! soundbar_running; then
    rm -f "$PIDFILE"
    /home/jorge/.cargo/bin/soundbar --bars 8 > "$FIFO" 2>&1 &
    echo $! > "$PIDFILE"
fi

# Leer el FIFO
# Usamos un bucle para mantener la conexión abierta
while read -r line < "$FIFO"; do
    # Mapeo de caracteres
    bars=" ▂▃▄▅▆▇█"
    output=""
    IFS=';' read -ra values <<< "$line"
    for v in "${values[@]}"; do
        if [ "$v" -ge 0 ] && [ "$v" -le 7 ]; then
            output+="${bars:$v:1}"
        else
            output+=" "
        fi
    done
    echo "$output"
done
