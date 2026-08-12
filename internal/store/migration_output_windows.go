//go:build windows

package store

import (
	"errors"
	"os"
)

type migrationOutputDirectory struct{}

func migrationOutputSupported() error {
	return errors.New("migration export is unsupported on Windows until private output ACL creation is implemented")
}

func openMigrationOutputDirectory(string) (*migrationOutputDirectory, error) {
	return nil, migrationOutputSupported()
}

func (*migrationOutputDirectory) createPartial() (*os.File, string, string, error) {
	return nil, "", "", migrationOutputSupported()
}

func (*migrationOutputDirectory) publish(string) error { return migrationOutputSupported() }

func (*migrationOutputDirectory) close() {}
