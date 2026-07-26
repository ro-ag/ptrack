package gitinfo

import (
	"bytes"
	"errors"
	"fmt"
	"strconv"
	"strings"
)

type Status struct {
	OID        string `json:"oid"`
	Branch     string `json:"branch"`
	Upstream   string `json:"upstream"`
	Detached   bool   `json:"detached"`
	Initial    bool   `json:"initial"`
	Ahead      int    `json:"ahead"`
	Behind     int    `json:"behind"`
	Staged     int    `json:"staged"`
	Unstaged   int    `json:"unstaged"`
	Untracked  int    `json:"untracked"`
	Conflicted int    `json:"conflicted"`
	Ignored    int    `json:"ignored"`
}

func parsePorcelainV2Status(input []byte) (Status, error) {
	var status Status
	records := bytes.Split(input, []byte{0})
	for index := 0; index < len(records); index++ {
		record := string(records[index])
		if record == "" {
			continue
		}
		switch {
		case strings.HasPrefix(record, "# "):
			if err := parseStatusHeader(&status, strings.TrimPrefix(record, "# ")); err != nil {
				return Status{}, err
			}
		case strings.HasPrefix(record, "1 "):
			fields := strings.SplitN(record, " ", 9)
			if len(fields) != 9 || len(fields[1]) != 2 {
				return Status{}, fmt.Errorf("malformed ordinary status record")
			}
			countXY(&status, fields[1])
		case strings.HasPrefix(record, "2 "):
			fields := strings.SplitN(record, " ", 10)
			if len(fields) != 10 || len(fields[1]) != 2 ||
				index+1 >= len(records) || len(records[index+1]) == 0 {
				return Status{}, fmt.Errorf("malformed renamed status record")
			}
			countXY(&status, fields[1])
			index++ // The second pathname is a separate NUL-delimited field.
		case strings.HasPrefix(record, "u "):
			fields := strings.SplitN(record, " ", 11)
			if len(fields) != 11 || len(fields[1]) != 2 {
				return Status{}, fmt.Errorf("malformed unmerged status record")
			}
			status.Conflicted++
		case strings.HasPrefix(record, "? "):
			if len(record) == 2 {
				return Status{}, errors.New("malformed untracked status record")
			}
			status.Untracked++
		case strings.HasPrefix(record, "! "):
			if len(record) == 2 {
				return Status{}, errors.New("malformed ignored status record")
			}
			status.Ignored++
		default:
			return Status{}, fmt.Errorf("unknown porcelain v2 record")
		}
	}
	return status, nil
}

func parseStatusHeader(status *Status, header string) error {
	key, value, found := strings.Cut(header, " ")
	if !found {
		return fmt.Errorf("malformed status header")
	}
	switch key {
	case "branch.oid":
		status.OID = value
	case "branch.head":
		status.Branch = value
		status.Detached = value == "(detached)"
		status.Initial = value == "(initial)"
	case "branch.upstream":
		status.Upstream = value
	case "branch.ab":
		fields := strings.Fields(value)
		if len(fields) != 2 || !strings.HasPrefix(fields[0], "+") ||
			!strings.HasPrefix(fields[1], "-") {
			return fmt.Errorf("malformed branch divergence header")
		}
		ahead, err := strconv.Atoi(strings.TrimPrefix(fields[0], "+"))
		if err != nil {
			return fmt.Errorf("parse ahead count: %w", err)
		}
		behind, err := strconv.Atoi(strings.TrimPrefix(fields[1], "-"))
		if err != nil {
			return fmt.Errorf("parse behind count: %w", err)
		}
		status.Ahead = ahead
		status.Behind = behind
	default:
		// Porcelain v2 allows clients to ignore headers they do not recognize.
	}
	return nil
}

func countXY(status *Status, xy string) {
	if xy[0] != '.' {
		status.Staged++
	}
	if xy[1] != '.' {
		status.Unstaged++
	}
}
