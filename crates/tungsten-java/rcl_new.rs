pub mod density_function {
    // Auto-generated inline Rust module from RCL
    #[allow(dead_code)]
    #[allow(unused_variables)]
    #[allow(unused_imports)]
    #[allow(warnings)]
    #[allow(clippy::all)]
    
    use super::*;
    use super::orchestration::*;
    
    const scale: f64 = (base * 2_f64);
    const base: f64 = 0.25_f64;
    const points: [f64; 3] = [-1_f64, 0_f64, f64::NAN];
    #[inline]
    fn minecraft_noise_d4x4x4(p: PositionIterator, config: &Config, buffers: &InternalBuffers, auto_constants: &AutoConsts, host_consts: &HostConsts) -> f64 {
        let mut v: f64;
        let mut bits: i64;
        let mut origin: Vec3 = config.origin;
        let mut pos3: Pos3 = p.pos;
        let mut origin_scale: Vec3 = config.origin_scale;
        let mut position_scale: Vec3 = config.position_scale;
        v = (floor((pos3.x * scale)) + sin(sample((&auto_constants.minecraft_temperature_0_), pos3.x)));
        bits = ((v as i64) * -5_i64);
        bits = nudge((&mut v));
        return (v + points[bits as usize]);
    }
    
    #[inline]
    fn minecraft_scalar_d1x1x1(p: PositionIterator, config: &Config, buffers: &InternalBuffers, auto_constants: &AutoConsts, host_consts: &HostConsts) -> i32 {
        let mut flag: bool;
        let mut origin: Vec3 = config.origin;
        let mut pos3: Pos3 = p.pos;
        let mut origin_scale: Vec3 = config.origin_scale;
        let mut position_scale: Vec3 = config.position_scale;
        flag = (host_consts.sea_level > 62_i32);
        return (((host_consts.sea_level + (flag as i32)) ^ (!host_consts.sea_level)) >> 1_i32);
    }
    
    #[inline]
    fn minecraft_final_d4x4x4(p: PositionIterator, config: &Config, buffers: &InternalBuffers, auto_constants: &AutoConsts, host_consts: &HostConsts) -> f64 {
        let mut same: bool;
        let mut origin: Vec3 = config.origin;
        let mut pos3: Pos3 = p.pos;
        let mut origin_scale: Vec3 = config.origin_scale;
        let mut position_scale: Vec3 = config.position_scale;
        same = (origin == Vec3::new(0_f64, 0_f64, 0_f64));
        if same {
            return buffers.minecraft_noise_d4x4x4_output[0_i32 as usize];
        }
        return (int(p, config, buffers, auto_constants, host_consts, buffers.minecraft_noise_d4x4x4_output[index(p) as usize]) + (buffers.minecraft_scalar_d1x1x1_output[0_i32 as usize] as f64));
    }
    
    #[inline]
    fn int(p: PositionIterator, config: &Config, buffers: &InternalBuffers, auto_constants: &AutoConsts, host_consts: &HostConsts, t: f64) -> f64 {
        let mut acc: f64;
        acc = 0_f64;
        {
            let mut repeat_counter: i32 = 0_i32;
            while (repeat_counter < 3_i32) {
                acc = (acc + t);
                if (acc > 10_f64) {
                    break;
                }
                repeat_counter = (repeat_counter + 1_i32);
            }
        }
        return acc;
    }
    
    pub fn minecraft_noise_d4x4x4_kernel(origin: Vec3, buffers: &mut InternalBuffers, auto_constants: &AutoConsts, host_consts: &HostConsts) {
        let config: Config = Config { origin: origin, origin_scale: Vec3::new(1_f64, 1_f64, 1_f64), position_scale: Vec3::new(0.25_f64, 0.25_f64, 0.25_f64), dimensions: (4_i32, 4_i32, 4_i32) };
        for p in iter_3d((&config)) {
            let r: f64 = minecraft_noise_d4x4x4(p, (&config), buffers, auto_constants, host_consts);
            buffers.minecraft_noise_d4x4x4_output[index(p) as usize] = r;
        }
    }
    
    pub fn minecraft_scalar_d1x1x1_kernel(origin: Vec3, buffers: &mut InternalBuffers, auto_constants: &AutoConsts, host_consts: &HostConsts) {
        let config: Config = Config { origin: origin, origin_scale: Vec3::new(1_f64, 1_f64, 1_f64), position_scale: Vec3::new(1_f64, 1_f64, 1_f64), dimensions: (1_i32, 1_i32, 1_i32) };
        for p in iter_3d((&config)) {
            let r: i32 = minecraft_scalar_d1x1x1(p, (&config), buffers, auto_constants, host_consts);
            buffers.minecraft_scalar_d1x1x1_output[index(p) as usize] = r;
        }
    }
    
    pub fn minecraft_final_d4x4x4_kernel(origin: Vec3, buffers: &mut InternalBuffers, auto_constants: &AutoConsts, host_consts: &HostConsts) {
        let config: Config = Config { origin: origin, origin_scale: Vec3::new(1_f64, 1_f64, 1_f64), position_scale: Vec3::new(0.25_f64, 0.25_f64, 0.25_f64), dimensions: (4_i32, 4_i32, 4_i32) };
        for p in iter_3d((&config)) {
            let r: f64 = minecraft_final_d4x4x4(p, (&config), buffers, auto_constants, host_consts);
            buffers.minecraft_final_d4x4x4_output[index(p) as usize] = r;
        }
    }
    
}


pub mod orchestration {
    // Auto-generated inline Rust module from RCL
    #[allow(dead_code)]
    #[allow(unused_variables)]
    #[allow(unused_imports)]
    #[allow(warnings)]
    #[allow(clippy::all)]
    
    use super::*;
    use super::density_function::*;
    
    #[repr(C)]
    pub struct HostConsts {
        pub sea_level: i32,
    }
    
    #[repr(C)]
    #[derive(Clone)]
    pub struct AutoConsts {
        pub minecraft_temperature_0_: PerlinNoiseSampler,
    }
    
    #[repr(C)]
    #[derive(Clone, Copy)]
    pub struct InternalBuffers {
        pub minecraft_final_d4x4x4_output: [f64; 64],
        pub minecraft_noise_d4x4x4_output: [f64; 64],
        pub minecraft_scalar_d1x1x1_output: [i32; 1],
    }
    
    #[derive(Clone)]
    pub struct Orchestration {
        pub buffers: Box<InternalBuffers>,
        pub auto_constants: Box<AutoConsts>,
    }
    
    fn make_auto_consts(seed: i64, auto_consts: &mut AutoConsts) {
        auto_consts.minecraft_temperature_0_ = make_permutation_table(seed, 6664882324328353151_i64, -587597586455377528_i64, 0_i64, 0_i64, 0_i64);
    }
    
    impl Orchestration {
        pub fn new(seed: i64) -> Self {
            let mut buffers: Box<InternalBuffers> = make_boxed::<InternalBuffers>();
            let mut auto_constants: Box<AutoConsts> = make_boxed::<AutoConsts>();
            let _init: () = make_auto_consts(seed, (&mut auto_constants));
            return Self { buffers: buffers, auto_constants: auto_constants };
        }
        
        pub fn orchestrate_minecraft_final(&mut self, origin: Vec3, host_consts: &HostConsts) -> &[f64; 64] {
            let _kernel_call_0_0: () = minecraft_noise_d4x4x4_kernel(origin, (&mut (*self.buffers)), (&(*self.auto_constants)), host_consts);
            let _kernel_call_0_1: () = minecraft_scalar_d1x1x1_kernel(origin, (&mut (*self.buffers)), (&(*self.auto_constants)), host_consts);
            let _kernel_call_1_0: () = minecraft_final_d4x4x4_kernel(origin, (&mut (*self.buffers)), (&(*self.auto_constants)), host_consts);
            return (&self.buffers.minecraft_final_d4x4x4_output);
        }
    }
    
}
