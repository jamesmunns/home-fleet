target remote :2335
set backtrace limit 32
load
monitor reset
break main
continue
