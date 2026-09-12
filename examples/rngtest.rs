use simulation::rng::Rng;
fn main() {
    let mut r = Rng::new(7);
    let mut heads = 0;
    for _ in 0..1000 {
        if r.f64() < 0.5 {
            heads += 1;
        }
    }
    println!("heads {heads}/1000");
    let mut r = Rng::new(7);
    let mut lo = 0;
    for _ in 0..1000 {
        if r.below(1500) < 750 {
            lo += 1;
        }
    }
    println!("below1500 lo {lo}/1000");
}
