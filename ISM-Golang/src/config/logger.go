package config

import (
	"log"
	"os"
)

var Log = log.New(os.Stdout, "[ISM-GLOBAL] ", log.Ldate|log.Ltime|log.Lshortfile)
