package config

import (
	"github.com/joho/godotenv"
	"os"
	"strings"
)

type IsmConfig struct {
	Host     string
	Port     string
	EnvMode  string
	DbConfig struct {
		DbHost     string
		DbPort     string
		DbUser     string
		DbPassword string
		DbKeyspace string
		DbInit     bool
	}
}

var (
	Config = &IsmConfig{}
)

func init() {
	Log.Println("Loading environment configuration...")
	err := godotenv.Load()
	if err != nil {
		Log.Fatal("Error loading .env file")
	}
	Config = &IsmConfig{
		Host:    GetEnv("HOST", "localhost"),
		Port:    GetEnv("PORT", "8080"),
		EnvMode: GetEnv("ENV", "development"),
		DbConfig: struct {
			DbHost     string
			DbPort     string
			DbUser     string
			DbPassword string
			DbKeyspace string
			DbInit     bool
		}{
			DbHost:     GetEnv("DB_HOST", "localhost"),
			DbPort:     GetEnv("DB_PORT", "5432"),
			DbUser:     GetEnv("DB_USER", "admin"),
			DbPassword: GetEnv("DB_PASSWORD", "admin"),
			DbKeyspace: GetEnv("DB_KEYSPACE", "messaging"),
			DbInit:     GetEnvAsBool("DB_INIT", true),
		},
	}
}

func GetEnv(key, defaultValue string) string {
	value := os.Getenv(key)
	if value == "" {
		return defaultValue
	}
	return value
}

func GetEnvAsBool(key string, defaultValue bool) bool {
	value := os.Getenv(key)
	if value == "" {
		return defaultValue
	}
	value = strings.ToLower(value)
	return value == "true" || value == "1" || value == "yes" || value == "on"
}
