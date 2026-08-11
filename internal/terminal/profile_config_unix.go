//go:build !windows

package terminal

import "os"

func replaceProfileConfig(temporaryPath, path string) error {
	return os.Rename(temporaryPath, path)
}

func syncProfileConfigDirectory(path string) error {
	directory, err := os.Open(path)
	if err != nil {
		return err
	}
	defer directory.Close()
	return directory.Sync()
}
