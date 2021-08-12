mod common;
use common::*;

#[test]
fn check_node_comb_1() {
    let (node_conf, mut node_exec) = new_node_engine();
    let mut matrix = Matrix::new(node_conf, 3, 3);

    let noise_1 = NodeId::Noise(0);
    let comb_1  = NodeId::Comb(0);
    let out_1   = NodeId::Out(0);
    matrix.place(0, 1,
        Cell::empty(noise_1)
        .input(None, None, None)
        .out(None, noise_1.out("sig"), None));
    matrix.place(1, 1,
        Cell::empty(comb_1)
        .input(None, comb_1.inp("inp"), None)
        .out(None, comb_1.out("sig"), None));
    matrix.place(2, 2,
        Cell::empty(out_1)
        .input(None, out_1.inp("ch1"), out_1.inp("ch1"))
        .out(None, None, None));

    pset_n(&mut matrix, comb_1, "g", 0.950);
    pset_n(&mut matrix, comb_1, "time", 0.014);
    matrix.sync().unwrap();

    let fft = run_and_get_avg_fft4096_now(&mut node_exec, 180);
    assert_eq!(fft, vec![
        (0, 216), (11, 221), (22, 216), (3370, 206), (3381, 248),
        (3391, 191), (6740, 185), (6751, 207), (6761, 195), (10131, 215),
        (10142, 210), (10153, 213), (10164, 201), (20338, 187), (20349, 184)
    ]);

    pset_n_wait(&mut matrix, &mut node_exec, comb_1, "time", 0.030);

    let fft = run_and_get_avg_fft4096_now(&mut node_exec, 180);
    assert_eq!(fft, vec![
        (1001, 206), (2993, 196), (3004, 219), (3994, 197),
        (6998, 211), (8000, 201)
    ]);

    pset_n_wait(&mut matrix, &mut node_exec, comb_1, "g", 0.999);
    let fft = run_and_get_avg_fft4096_now(&mut node_exec, 1000);
    assert_eq!(fft, vec![
        (0, 2003), (11, 1015), (991, 1078), (1001, 1837), (2003, 1059),
        (2993, 1420), (3004, 1775), (3994, 1297), (4005, 1485)
    ]);
}
