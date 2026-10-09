package remote

import (
	"crypto/ecdh"
	"encoding/hex"
	"testing"
)

// RFC 8291 appendix A: a known message, keys, auth secret and salt, and
// the body the RFC says they make.
func TestSealRFC8291(t *testing.T) {
	d := func(s string) []byte {
		b, err := b64.DecodeString(s)
		if err != nil {
			t.Fatal(err)
		}
		return b
	}
	as, err := ecdh.P256().NewPrivateKey(d("yfWPiYE-n46HLnH0KqZOF1fJJU3MYrct3AELtAQ-oRw"))
	if err != nil {
		t.Fatal(err)
	}
	ua, err := ecdh.P256().NewPublicKey(d("BCVxsr7N_eNgVRqvHtD0zTZsEc6-VV-JvLexhqUzORcxaOzi6-AYWXvTBHm4bjyPjs7Vd8pZGH6SRpkNtoIAiw4"))
	if err != nil {
		t.Fatal(err)
	}
	got, err := seal(as, ua, d("BTBZMqHH6r4Tts7J_aSIgg"), d("DGv6ra1nlYgDCS1FRnbzlw"), []byte("When I grow up, I want to be a watermelon"))
	if err != nil {
		t.Fatal(err)
	}
	want := d("DGv6ra1nlYgDCS1FRnbzlwAAEABBBP4z9KsN6nGRTbVYI_c7VJSPQTBtkgcy27mlmlMoZIIgDll6e3vCYLocInmYWAmS6TlzAC8wEqKK6PBru3jl7A_yl95bQpu6cVPTpK4Mqgkf1CXztLVBSt2Ks3oZwbuwXPXLWyouBWLVWGNWQexSgSxsj_Qulcy4a-fN")
	if hex.EncodeToString(got) != hex.EncodeToString(want) {
		t.Fatalf("got  %x\nwant %x", got, want)
	}
}

func TestPushHostOK(t *testing.T) {
	for u, want := range map[string]bool{
		"https://fcm.googleapis.com/fcm/send/x":       true,
		"https://web.push.apple.com/abc":              true,
		"https://updates.push.services.mozilla.com/x": true,
		"http://fcm.googleapis.com/x":                 false,
		"https://127.0.0.1/x":                         false,
		"https://evil.com/.push.apple.com":            false,
		"https://x.push.apple.com.evil.com/":          false,
		"https://user@fcm.googleapis.com/":            false,
		"https://fcm.googleapis.com:8443/":            false,
	} {
		if pushHostOK(u) != want {
			t.Errorf("%s: want %v", u, want)
		}
	}
}
