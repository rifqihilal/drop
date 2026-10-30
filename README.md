# Drop

Notch ringan untuk Windows 10 dan 11. Duduk di tepi atas layar, mengecil saat
komputer dipakai, dan melebar saat ada yang perlu dilihat.

Ditulis dalam Rust langsung di atas Win32, Direct2D, dan DirectWrite, tanpa
webview. Memakai sekitar 9 MB RAM saat diam dan sekitar 14 MB saat musik dan
lirik tampil.

## Fitur

**Notch tertutup**

- Cincin baterai, jam, dan suhu.
- Mata beranimasi saat komputer tidak disentuh beberapa menit.
- Info langsung di samping jam: musik, timer Pomodoro, atau sholat yang sudah dekat.
- Bersembunyi saat ada aplikasi layar penuh.

**Kartu** (arahkan mouse ke notch, lalu scroll, klik panah samping, atau tekan `Alt+N`)

- **Hari Ini:** cuaca, sholat berikutnya, dan baterai.
- **Sedang Diputar:** kontrol musik, sampul album, dan lirik tersinkron dari [LRCLIB](https://lrclib.net).
- **Prakiraan Cuaca:** enam jam ke depan dan tujuh hari, dari [Open-Meteo](https://open-meteo.com).
- **Waktu Sholat:** jadwal Kemenag RI dan arah kiblat.
- **Kalender & Fokus:** kalender bulanan dan Pomodoro.
- **Agenda Acara:** acara dari tautan kalender `.ics`, dengan pengingat 10 menit sebelumnya.
- **Daftar Tugas:** tugas singkat yang diketik langsung di notch.
- **Rak Berkas:** seret berkas ke notch, lalu seret keluar ke aplikasi lain.

**Notifikasi di notch**

- Volume dan kecerahan, menggantikan tampilan bawaan Windows.
- Wi-Fi tersambung atau terputus, perangkat Bluetooth dengan persen baterainya, dan baterai lemah.
- Pengingat sholat, minum air, istirahat mata, dan peregangan.

Setiap modul bisa dimatikan di Pengaturan (klik kanan notch atau ikon tray).

## Membangun

Butuh [Rust](https://rustup.rs) stable di Windows.

```bash
cargo build --release
```

Hasilnya ada di `target/release/drop.exe`.

Pilihan baris perintah:

| Pilihan | Fungsi |
| --- | --- |
| `--settings` | Membuka Pengaturan saat mulai |
| `--idle-secs N` | Memunculkan mata setelah N detik, mengabaikan pengaturan |
| `--demo` | Menampilkan contoh semua notifikasi |

## Data

Semua tersimpan di `%APPDATA%\Drop`, berupa berkas teks yang bisa disunting:

| Berkas | Isi |
| --- | --- |
| `drop.ini` | Pengaturan; dibaca ulang dalam satu menit setelah diubah |
| `todo.txt` | Daftar tugas |
| `shelf.txt` | Lokasi berkas di rak (hanya lokasi, berkasnya tidak disalin) |

Drop menghubungi layanan luar hanya untuk fitur yang memerlukannya:

- **Open-Meteo:** koordinat lokasi untuk cuaca dan nama tempat saat mencari lokasi.
- **LRCLIB:** judul lagu dan nama penyanyi untuk mencari lirik.
- **Tautan `.ics` milikmu:** untuk mengambil agenda.

Modul Cuaca dan Lirik lagu bisa dimatikan di Pengaturan. Agenda hanya aktif kalau
tautannya diisi.

## Asal-usul dan lisensi


Dilisensikan di bawah [GPLv3](LICENSE).
