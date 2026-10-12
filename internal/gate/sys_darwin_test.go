package gate

import "testing"

func TestParseVMStat(t *testing.T) {
	out := "Mach Virtual Memory Statistics: (page size of 16384 bytes)\n" +
		"Pages free:                                   100.\nPages active:                                 999.\n" +
		"Pages inactive:                                 20.\nPages speculative:                              7.\n" +
		"Pages purgeable:                                 3.\n"
	if got, ok := parseVMStat(out); !ok || got != 123*16384 {
		t.Fatalf("parseVMStat = %d, %v", got, ok)
	}
	if _, ok := parseVMStat("nonsense"); ok {
		t.Fatal("read nonsense")
	}
}
